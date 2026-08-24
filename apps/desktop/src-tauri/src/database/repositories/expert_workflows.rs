use super::{now_ms, Database};
use crate::database::{
    ExpertWorkflowGateRecord, ExpertWorkflowRunRecord, ExpertWorkflowSnapshot,
    ExpertWorkflowStageRunRecord, StartExpertWorkflowInput,
};
use rusqlite::{params, OptionalExtension, Row};
use serde_json::{json, Value};
use uuid::Uuid;

impl Database {
    pub fn start_expert_workflow(
        &self,
        input: &StartExpertWorkflowInput,
    ) -> Result<ExpertWorkflowSnapshot, String> {
        if input.stages.is_empty() {
            return Err("workflow.definition_empty".to_owned());
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let active_workflow = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM expert_workflow_runs
                    WHERE conversation_id = ?1 AND status IN ('running', 'awaiting_gate')
                 )",
                [&input.conversation_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())?;
        if active_workflow {
            return Err("workflow.already_active".to_owned());
        }
        let active_goal = transaction
            .query_row(
                "SELECT id, status, created_by,
                        (SELECT COUNT(*) FROM work_tasks WHERE goal_id = goals.id)
                 FROM goals
                 WHERE conversation_id = ?1 AND status IN ('active', 'blocked')
                 ORDER BY created_at DESC LIMIT 1",
                [&input.conversation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let now = now_ms();
        let timestamp = now.to_string();
        let workflow_run_id = format!("expert-workflow-{}", Uuid::new_v4().simple());
        let workflow_owner = format!("workflow:{}@{}", input.workflow_id, input.workflow_version);
        let goal_id = match active_goal {
            Some((goal_id, status, created_by, task_count))
                if status == "active" && created_by == "host:work-mode-gate" && task_count == 0 =>
            {
                transaction
                    .execute(
                        "UPDATE goals
                         SET title = ?2, objective = ?3, acceptance_summary = ?4,
                             created_by = ?5, version = version + 1, updated_at = ?6
                         WHERE id = ?1",
                        params![
                            goal_id,
                            input.title,
                            input.objective,
                            input.acceptance_summary,
                            workflow_owner,
                            timestamp
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                goal_id
            }
            Some(_) => return Err("workflow.active_goal_conflict".to_owned()),
            None => {
                let goal_id = format!("workflow-goal-{}", Uuid::new_v4().simple());
                transaction
                    .execute(
                        "INSERT INTO goals(
                            id, conversation_id, title, objective, acceptance_summary, status,
                            version, created_by, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', 1, ?6, ?7, ?7)",
                        params![
                            goal_id,
                            input.conversation_id,
                            input.title,
                            input.objective,
                            input.acceptance_summary,
                            workflow_owner,
                            timestamp
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                goal_id
            }
        };
        let first_requires_gate = input.stages[0].user_gate;
        transaction
            .execute(
                "INSERT INTO expert_workflow_runs(
                    id, conversation_id, expert_binding_id, expert_id, package_hash,
                    workflow_id, workflow_version, workflow_json, goal_id, status,
                    current_stage_index, input_json, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12, ?12)",
                params![
                    workflow_run_id,
                    input.conversation_id,
                    input.expert_binding_id,
                    input.expert_id,
                    input.package_hash,
                    input.workflow_id,
                    input.workflow_version,
                    input.workflow.to_string(),
                    goal_id,
                    if first_requires_gate {
                        "awaiting_gate"
                    } else {
                        "running"
                    },
                    input.input.to_string(),
                    now
                ],
            )
            .map_err(|error| error.to_string())?;
        for (index, stage) in input.stages.iter().enumerate() {
            let task_id = format!("workflow-task-{}", Uuid::new_v4().simple());
            transaction
                .execute(
                    "INSERT INTO work_tasks(
                        id, goal_id, ordinal, title, detail, status, attempt,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', 1, ?6, ?6)",
                    params![
                        task_id,
                        goal_id,
                        index as i64,
                        stage.title,
                        stage.detail,
                        timestamp
                    ],
                )
                .map_err(|error| error.to_string())?;
            let stage_status = if index == 0 {
                if stage.user_gate {
                    "awaiting_gate"
                } else {
                    "queued"
                }
            } else {
                "pending"
            };
            transaction
                .execute(
                    "INSERT INTO expert_workflow_stage_runs(
                        id, workflow_run_id, stage_id, task_id, ordinal, status,
                        attempt, max_attempts, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
                    params![
                        format!("workflow-stage-{}", Uuid::new_v4().simple()),
                        workflow_run_id,
                        stage.stage_id,
                        task_id,
                        index as i64,
                        stage_status,
                        stage.max_attempts,
                        now
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        if first_requires_gate {
            insert_pending_gate(
                &transaction,
                &workflow_run_id,
                &input.stages[0].stage_id,
                "stage requires user approval before execution",
                now,
            )?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        load_workflow_snapshot(&connection, &workflow_run_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow.not_found".to_owned())
    }

    pub fn active_expert_workflow(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ExpertWorkflowSnapshot>, String> {
        self.with_connection(|connection| {
            let id = connection
                .query_row(
                    "SELECT id FROM expert_workflow_runs
                     WHERE conversation_id = ?1 AND status IN ('running', 'awaiting_gate')
                     ORDER BY created_at DESC LIMIT 1",
                    [conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            id.map(|id| load_workflow_snapshot(connection, &id))
                .transpose()
                .map(Option::flatten)
        })
    }

    pub fn get_expert_workflow(
        &self,
        workflow_run_id: &str,
    ) -> Result<Option<ExpertWorkflowSnapshot>, String> {
        self.with_connection(|connection| load_workflow_snapshot(connection, workflow_run_id))
    }

    pub fn start_expert_workflow_stage(
        &self,
        workflow_run_id: &str,
        stage_id: &str,
        owner_run_id: &str,
    ) -> Result<ExpertWorkflowSnapshot, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let (status, current_stage_index) = workflow_state(&transaction, workflow_run_id)?;
        if status != "running" {
            return Err("workflow.not_runnable".to_owned());
        }
        let (task_id, ordinal, stage_status, attempt, max_attempts) = transaction
            .query_row(
                "SELECT task_id, ordinal, status, attempt, max_attempts
                 FROM expert_workflow_stage_runs
                 WHERE workflow_run_id = ?1 AND stage_id = ?2",
                params![workflow_run_id, stage_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(|_| "workflow.stage_not_found".to_owned())?;
        if ordinal != current_stage_index || stage_status != "queued" || attempt >= max_attempts {
            return Err("workflow.stage_not_runnable".to_owned());
        }
        let task_status = transaction
            .query_row(
                "SELECT status FROM work_tasks WHERE id = ?1",
                [&task_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| error.to_string())?;
        if !matches!(task_status.as_str(), "queued" | "interrupted") {
            return Err("workflow.task_state_conflict".to_owned());
        }
        let now = now_ms();
        let timestamp = now.to_string();
        transaction
            .execute(
                "UPDATE work_tasks
                 SET status = 'in_progress', owner_run_id = ?2,
                     attempt = CASE WHEN status = 'interrupted' THEN attempt + 1 ELSE attempt END,
                     version = version + 1, updated_at = ?3,
                     started_at = COALESCE(started_at, ?3), finished_at = NULL,
                     blocked_reason = NULL
                 WHERE id = ?1",
                params![task_id, owner_run_id, timestamp],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE expert_workflow_stage_runs
                 SET status = 'running', attempt = attempt + 1,
                     started_at = COALESCE(started_at, ?3), updated_at = ?3,
                     error_message = NULL
                 WHERE workflow_run_id = ?1 AND stage_id = ?2",
                params![workflow_run_id, stage_id, now],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE expert_workflow_runs SET updated_at = ?2, error_message = NULL WHERE id = ?1",
                params![workflow_run_id, now],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        load_workflow_snapshot(&connection, workflow_run_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow.not_found".to_owned())
    }

    pub fn complete_expert_workflow_stage(
        &self,
        workflow_run_id: &str,
        stage_id: &str,
        stage_output: &Value,
        workflow_output: Option<&Value>,
        next_user_gate: bool,
    ) -> Result<ExpertWorkflowSnapshot, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let (status, current_stage_index) = workflow_state(&transaction, workflow_run_id)?;
        if status != "running" {
            return Err("workflow.not_runnable".to_owned());
        }
        let (task_id, ordinal, stage_status) = transaction
            .query_row(
                "SELECT task_id, ordinal, status FROM expert_workflow_stage_runs
                 WHERE workflow_run_id = ?1 AND stage_id = ?2",
                params![workflow_run_id, stage_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|_| "workflow.stage_not_found".to_owned())?;
        if ordinal != current_stage_index || stage_status != "running" {
            return Err("workflow.stage_not_running".to_owned());
        }
        let valid_evidence = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM task_evidence
                    WHERE task_id = ?1 AND validity_status = 'valid'
                 )",
                [&task_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())?;
        if !valid_evidence {
            return Err("workflow.stage_evidence_required".to_owned());
        }
        let now = now_ms();
        let timestamp = now.to_string();
        transaction
            .execute(
                "UPDATE work_tasks
                 SET status = 'completed', version = version + 1, updated_at = ?2,
                     finished_at = ?2, blocked_reason = NULL
                 WHERE id = ?1 AND status = 'in_progress'",
                params![task_id, timestamp],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE expert_workflow_stage_runs
                 SET status = 'completed', output_json = ?3, error_message = NULL,
                     completed_at = ?4, updated_at = ?4
                 WHERE workflow_run_id = ?1 AND stage_id = ?2",
                params![workflow_run_id, stage_id, stage_output.to_string(), now],
            )
            .map_err(|error| error.to_string())?;
        let stage_count = transaction
            .query_row(
                "SELECT COUNT(*) FROM expert_workflow_stage_runs WHERE workflow_run_id = ?1",
                [workflow_run_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| error.to_string())?;
        if ordinal + 1 == stage_count {
            let output = workflow_output.ok_or_else(|| "workflow.output_required".to_owned())?;
            transaction
                .execute(
                    "UPDATE expert_workflow_runs
                     SET status = 'completed', output_json = ?2, error_message = NULL,
                         updated_at = ?3, completed_at = ?3
                     WHERE id = ?1",
                    params![workflow_run_id, output.to_string(), now],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "UPDATE goals
                     SET status = 'completed', version = version + 1,
                         updated_at = ?2, completed_at = ?2, blocked_reason = NULL
                     WHERE id = (SELECT goal_id FROM expert_workflow_runs WHERE id = ?1)",
                    params![workflow_run_id, timestamp],
                )
                .map_err(|error| error.to_string())?;
        } else {
            let next_index = ordinal + 1;
            let (next_stage_id, next_status) = transaction
                .query_row(
                    "SELECT stage_id, status FROM expert_workflow_stage_runs
                     WHERE workflow_run_id = ?1 AND ordinal = ?2",
                    params![workflow_run_id, next_index],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|error| error.to_string())?;
            if next_status != "pending" {
                return Err("workflow.next_stage_state_conflict".to_owned());
            }
            let next_status = if next_user_gate {
                "awaiting_gate"
            } else {
                "queued"
            };
            transaction
                .execute(
                    "UPDATE expert_workflow_stage_runs SET status = ?3, updated_at = ?4
                     WHERE workflow_run_id = ?1 AND stage_id = ?2",
                    params![workflow_run_id, next_stage_id, next_status, now],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "UPDATE expert_workflow_runs
                     SET status = ?2, current_stage_index = ?3, updated_at = ?4
                     WHERE id = ?1",
                    params![
                        workflow_run_id,
                        if next_user_gate {
                            "awaiting_gate"
                        } else {
                            "running"
                        },
                        next_index,
                        now
                    ],
                )
                .map_err(|error| error.to_string())?;
            if next_user_gate {
                insert_pending_gate(
                    &transaction,
                    workflow_run_id,
                    &next_stage_id,
                    "stage requires user approval before execution",
                    now,
                )?;
            }
        }
        transaction.commit().map_err(|error| error.to_string())?;
        load_workflow_snapshot(&connection, workflow_run_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow.not_found".to_owned())
    }

    pub fn fail_expert_workflow_stage(
        &self,
        workflow_run_id: &str,
        stage_id: &str,
        error_message: &str,
    ) -> Result<ExpertWorkflowSnapshot, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let (_, current_stage_index) = workflow_state(&transaction, workflow_run_id)?;
        let (task_id, ordinal, status, attempt, max_attempts) = transaction
            .query_row(
                "SELECT task_id, ordinal, status, attempt, max_attempts
                 FROM expert_workflow_stage_runs
                 WHERE workflow_run_id = ?1 AND stage_id = ?2",
                params![workflow_run_id, stage_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(|_| "workflow.stage_not_found".to_owned())?;
        if ordinal != current_stage_index || status != "running" {
            return Err("workflow.stage_not_running".to_owned());
        }
        let now = now_ms();
        let timestamp = now.to_string();
        transaction
            .execute(
                "UPDATE work_tasks
                 SET status = ?2, version = version + 1, updated_at = ?3,
                     finished_at = ?3, blocked_reason = CASE WHEN ?2 = 'blocked' THEN ?4 ELSE NULL END
                 WHERE id = ?1",
                params![
                    task_id,
                    if attempt < max_attempts {
                        "interrupted"
                    } else {
                        "blocked"
                    },
                    timestamp,
                    error_message
                ],
            )
            .map_err(|error| error.to_string())?;
        if attempt < max_attempts {
            transaction
                .execute(
                    "UPDATE expert_workflow_stage_runs
                     SET status = 'queued', error_message = ?3, updated_at = ?4
                     WHERE workflow_run_id = ?1 AND stage_id = ?2",
                    params![workflow_run_id, stage_id, error_message, now],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "UPDATE expert_workflow_runs SET status = 'running', error_message = ?2,
                         updated_at = ?3 WHERE id = ?1",
                    params![workflow_run_id, error_message, now],
                )
                .map_err(|error| error.to_string())?;
        } else {
            transaction
                .execute(
                    "UPDATE expert_workflow_stage_runs
                     SET status = 'failed', error_message = ?3, completed_at = ?4, updated_at = ?4
                     WHERE workflow_run_id = ?1 AND stage_id = ?2",
                    params![workflow_run_id, stage_id, error_message, now],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "UPDATE expert_workflow_runs SET status = 'failed', error_message = ?2,
                         updated_at = ?3, completed_at = ?3 WHERE id = ?1",
                    params![workflow_run_id, error_message, now],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "UPDATE goals SET status = 'blocked', version = version + 1,
                         blocked_reason = ?2, updated_at = ?3
                     WHERE id = (SELECT goal_id FROM expert_workflow_runs WHERE id = ?1)",
                    params![workflow_run_id, error_message, timestamp],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        load_workflow_snapshot(&connection, workflow_run_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow.not_found".to_owned())
    }

    pub fn resolve_expert_workflow_gate(
        &self,
        workflow_run_id: &str,
        stage_id: &str,
        approve: bool,
        reason: &str,
        resolved_by: &str,
    ) -> Result<ExpertWorkflowSnapshot, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let (workflow_status, current_stage_index) = workflow_state(&transaction, workflow_run_id)?;
        if workflow_status != "awaiting_gate" {
            return Err("workflow.gate_not_pending".to_owned());
        }
        let (task_id, ordinal, stage_status) = transaction
            .query_row(
                "SELECT task_id, ordinal, status FROM expert_workflow_stage_runs
                 WHERE workflow_run_id = ?1 AND stage_id = ?2",
                params![workflow_run_id, stage_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|_| "workflow.stage_not_found".to_owned())?;
        if ordinal != current_stage_index || stage_status != "awaiting_gate" {
            return Err("workflow.gate_stage_conflict".to_owned());
        }
        let now = now_ms();
        let changed = transaction
            .execute(
                "UPDATE expert_workflow_gate_decisions
                 SET status = ?3, reason = ?4, resolved_at = ?5, resolved_by = ?6
                 WHERE workflow_run_id = ?1 AND stage_id = ?2 AND status = 'pending'",
                params![
                    workflow_run_id,
                    stage_id,
                    if approve { "approved" } else { "rejected" },
                    reason,
                    now,
                    resolved_by
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("workflow.gate_not_pending".to_owned());
        }
        if approve {
            transaction
                .execute(
                    "UPDATE expert_workflow_stage_runs SET status = 'queued', updated_at = ?3
                     WHERE workflow_run_id = ?1 AND stage_id = ?2",
                    params![workflow_run_id, stage_id, now],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "UPDATE expert_workflow_runs SET status = 'running', updated_at = ?2
                     WHERE id = ?1",
                    params![workflow_run_id, now],
                )
                .map_err(|error| error.to_string())?;
        } else {
            cancel_workflow_transaction(
                &transaction,
                workflow_run_id,
                Some((&task_id, stage_id)),
                reason,
                now,
            )?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        load_workflow_snapshot(&connection, workflow_run_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow.not_found".to_owned())
    }

    pub fn cancel_expert_workflow(
        &self,
        workflow_run_id: &str,
        reason: &str,
    ) -> Result<ExpertWorkflowSnapshot, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let (status, _) = workflow_state(&transaction, workflow_run_id)?;
        if !matches!(status.as_str(), "running" | "awaiting_gate") {
            return Err("workflow.already_terminal".to_owned());
        }
        let now = now_ms();
        cancel_workflow_transaction(&transaction, workflow_run_id, None, reason, now)?;
        transaction.commit().map_err(|error| error.to_string())?;
        load_workflow_snapshot(&connection, workflow_run_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow.not_found".to_owned())
    }
}

fn workflow_state(
    connection: &rusqlite::Connection,
    workflow_run_id: &str,
) -> Result<(String, i64), String> {
    connection
        .query_row(
            "SELECT status, current_stage_index FROM expert_workflow_runs WHERE id = ?1",
            [workflow_run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "workflow.not_found".to_owned())
}

fn insert_pending_gate(
    transaction: &rusqlite::Transaction<'_>,
    workflow_run_id: &str,
    stage_id: &str,
    reason: &str,
    now: i64,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO expert_workflow_gate_decisions(
                id, workflow_run_id, stage_id, status, reason, requested_at
             ) VALUES (?1, ?2, ?3, 'pending', ?4, ?5)",
            params![
                format!("workflow-gate-{}", Uuid::new_v4().simple()),
                workflow_run_id,
                stage_id,
                reason,
                now
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn cancel_workflow_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workflow_run_id: &str,
    rejected_stage: Option<(&str, &str)>,
    reason: &str,
    now: i64,
) -> Result<(), String> {
    let timestamp = now.to_string();
    transaction
        .execute(
            "UPDATE expert_workflow_runs SET status = 'cancelled', error_message = ?2,
                 updated_at = ?3, completed_at = ?3 WHERE id = ?1",
            params![workflow_run_id, reason, now],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE expert_workflow_stage_runs
             SET status = CASE WHEN status = 'running' THEN 'failed' ELSE 'skipped' END,
                 error_message = ?2, completed_at = ?3, updated_at = ?3
             WHERE workflow_run_id = ?1 AND status NOT IN ('completed', 'failed', 'skipped')",
            params![workflow_run_id, reason, now],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE work_tasks
             SET status = CASE WHEN status = 'in_progress' THEN 'interrupted' ELSE 'skipped' END,
                 version = version + 1, updated_at = ?2, finished_at = ?2,
                 blocked_reason = NULL
             WHERE id IN (
                SELECT task_id FROM expert_workflow_stage_runs WHERE workflow_run_id = ?1
             ) AND status NOT IN ('completed', 'skipped')",
            params![workflow_run_id, timestamp],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE goals SET status = 'cancelled', version = version + 1,
                 updated_at = ?2, blocked_reason = NULL
             WHERE id = (SELECT goal_id FROM expert_workflow_runs WHERE id = ?1)",
            params![workflow_run_id, timestamp],
        )
        .map_err(|error| error.to_string())?;
    if let Some((task_id, stage_id)) = rejected_stage {
        let _ = (task_id, stage_id);
    }
    Ok(())
}

fn load_workflow_snapshot(
    connection: &rusqlite::Connection,
    workflow_run_id: &str,
) -> rusqlite::Result<Option<ExpertWorkflowSnapshot>> {
    let run = connection
        .query_row(
            "SELECT id, conversation_id, expert_binding_id, expert_id, package_hash,
                    workflow_id, workflow_version, workflow_json, goal_id, status,
                    current_stage_index, input_json, output_json, error_message,
                    created_at, updated_at, completed_at
             FROM expert_workflow_runs WHERE id = ?1",
            [workflow_run_id],
            workflow_run_from_row,
        )
        .optional()?;
    let Some(run) = run else {
        return Ok(None);
    };
    let stages = connection
        .prepare(
            "SELECT id, workflow_run_id, stage_id, task_id, ordinal, status,
                    attempt, max_attempts, output_json, error_message,
                    started_at, completed_at, updated_at
             FROM expert_workflow_stage_runs
             WHERE workflow_run_id = ?1 ORDER BY ordinal",
        )?
        .query_map([workflow_run_id], workflow_stage_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let gates = connection
        .prepare(
            "SELECT id, workflow_run_id, stage_id, status, reason,
                    requested_at, resolved_at, resolved_by
             FROM expert_workflow_gate_decisions
             WHERE workflow_run_id = ?1 ORDER BY requested_at, id",
        )?
        .query_map([workflow_run_id], workflow_gate_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(ExpertWorkflowSnapshot { run, stages, gates }))
}

fn workflow_run_from_row(row: &Row<'_>) -> rusqlite::Result<ExpertWorkflowRunRecord> {
    Ok(ExpertWorkflowRunRecord {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        expert_binding_id: row.get(2)?,
        expert_id: row.get(3)?,
        package_hash: row.get(4)?,
        workflow_id: row.get(5)?,
        workflow_version: row.get(6)?,
        workflow: parse_json(row.get(7)?),
        goal_id: row.get(8)?,
        status: row.get(9)?,
        current_stage_index: row.get(10)?,
        input: parse_json(row.get(11)?),
        output: row.get::<_, Option<String>>(12)?.map(parse_json),
        error_message: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        completed_at: row.get(16)?,
    })
}

fn workflow_stage_from_row(row: &Row<'_>) -> rusqlite::Result<ExpertWorkflowStageRunRecord> {
    Ok(ExpertWorkflowStageRunRecord {
        id: row.get(0)?,
        workflow_run_id: row.get(1)?,
        stage_id: row.get(2)?,
        task_id: row.get(3)?,
        ordinal: row.get(4)?,
        status: row.get(5)?,
        attempt: row.get(6)?,
        max_attempts: row.get(7)?,
        output: row.get::<_, Option<String>>(8)?.map(parse_json),
        error_message: row.get(9)?,
        started_at: row.get(10)?,
        completed_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn workflow_gate_from_row(row: &Row<'_>) -> rusqlite::Result<ExpertWorkflowGateRecord> {
    Ok(ExpertWorkflowGateRecord {
        id: row.get(0)?,
        workflow_run_id: row.get(1)?,
        stage_id: row.get(2)?,
        status: row.get(3)?,
        reason: row.get(4)?,
        requested_at: row.get(5)?,
        resolved_at: row.get(6)?,
        resolved_by: row.get(7)?,
    })
}

fn parse_json(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{
        AddEvidenceInput, CreateGoalInput, EvidenceReferenceKind, EvidenceType, GoalStatus,
        StartExpertWorkflowStageInput,
    };
    use std::path::PathBuf;

    fn test_database() -> (Database, PathBuf) {
        let path = std::env::temp_dir().join(format!("fox-expert-workflow-{}.db", Uuid::new_v4()));
        (
            Database::open(path.clone()).expect("open workflow database"),
            path,
        )
    }

    fn workflow_input(
        conversation_id: &str,
        binding_id: &str,
        package_hash: &str,
        first_gate: bool,
        second_gate: bool,
        max_attempts: i64,
    ) -> StartExpertWorkflowInput {
        StartExpertWorkflowInput {
            conversation_id: conversation_id.to_owned(),
            expert_binding_id: binding_id.to_owned(),
            expert_id: "fox-debugger".to_owned(),
            package_hash: package_hash.to_owned(),
            workflow_id: "debug.delivery".to_owned(),
            workflow_version: "1.0.0".to_owned(),
            workflow: json!({ "schemaVersion": 1, "id": "debug.delivery" }),
            title: "Debug delivery".to_owned(),
            objective: "Produce a verified diagnosis and repair".to_owned(),
            acceptance_summary: "Every stage has valid evidence".to_owned(),
            input: json!({ "issue": "broken build" }),
            stages: vec![
                StartExpertWorkflowStageInput {
                    stage_id: "diagnose".to_owned(),
                    title: "Diagnose".to_owned(),
                    detail: "Find the root cause".to_owned(),
                    max_attempts,
                    user_gate: first_gate,
                },
                StartExpertWorkflowStageInput {
                    stage_id: "verify".to_owned(),
                    title: "Verify".to_owned(),
                    detail: "Verify the repair".to_owned(),
                    max_attempts,
                    user_gate: second_gate,
                },
            ],
        }
    }

    fn external_evidence(task_id: &str, suffix: &str) -> AddEvidenceInput {
        AddEvidenceInput {
            id: None,
            task_id: task_id.to_owned(),
            source_run_id: None,
            evidence_type: EvidenceType::ExternalReference,
            ref_kind: EvidenceReferenceKind::Source,
            ref_id: format!("https://example.com/workflow/{suffix}"),
            summary: "verified workflow evidence".to_owned(),
            metadata: json!({ "source": "workflow-test" }),
            trace_id: None,
            span_id: None,
        }
    }

    fn cleanup(database: Database, path: PathBuf) {
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn persists_gates_checkpoints_and_reuses_the_empty_host_goal() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("workflow"), None, None)
            .expect("create conversation");
        let binding = database
            .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
            .expect("bind workflow expert");
        let owner_run = database
            .create_run(&conversation.id, "run expert workflow", None)
            .expect("create owner run");
        let placeholder = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "Pending work".to_owned(),
                objective: "Await workflow definition".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: "host:work-mode-gate".to_owned(),
            })
            .expect("create host placeholder goal");

        let snapshot = database
            .start_expert_workflow(&workflow_input(
                &conversation.id,
                &binding.id,
                &binding.package_hash,
                true,
                true,
                2,
            ))
            .expect("start workflow");
        assert_eq!(snapshot.run.goal_id, placeholder.id);
        assert_eq!(snapshot.run.status, "awaiting_gate");
        assert_eq!(snapshot.stages[0].status, "awaiting_gate");
        assert_eq!(snapshot.gates[0].status, "pending");
        let workflow_run_id = snapshot.run.id.clone();

        drop(database);
        let database = Database::open(path.clone()).expect("reopen workflow database");
        let restored = database
            .active_expert_workflow(&conversation.id)
            .expect("load active workflow")
            .expect("persisted workflow");
        assert_eq!(restored.run.id, workflow_run_id);
        assert_eq!(restored.stages.len(), 2);

        let approved = database
            .resolve_expert_workflow_gate(&workflow_run_id, "diagnose", true, "approved", "user")
            .expect("approve first gate");
        assert_eq!(approved.stages[0].status, "queued");
        let running = database
            .start_expert_workflow_stage(&workflow_run_id, "diagnose", &owner_run.run.id)
            .expect("start first stage");
        assert_eq!(running.stages[0].attempt, 1);
        let diagnosis_evidence = database
            .task_evidence()
            .add(external_evidence(&running.stages[0].task_id, "diagnosis"))
            .expect("record diagnosis evidence");
        database
            .task_evidence()
            .validate(&diagnosis_evidence.id)
            .expect("validate diagnosis evidence");
        let checkpoint = database
            .complete_expert_workflow_stage(
                &workflow_run_id,
                "diagnose",
                &json!({ "rootCause": "configuration drift" }),
                None,
                true,
            )
            .expect("checkpoint first stage");
        assert_eq!(checkpoint.run.status, "awaiting_gate");
        assert_eq!(checkpoint.run.current_stage_index, 1);
        assert_eq!(checkpoint.stages[0].status, "completed");
        assert_eq!(checkpoint.stages[1].status, "awaiting_gate");

        database
            .resolve_expert_workflow_gate(&workflow_run_id, "verify", true, "approved", "user")
            .expect("approve verification gate");
        let running = database
            .start_expert_workflow_stage(&workflow_run_id, "verify", &owner_run.run.id)
            .expect("start second stage");
        let verification_evidence = database
            .task_evidence()
            .add(external_evidence(
                &running.stages[1].task_id,
                "verification",
            ))
            .expect("record verification evidence");
        database
            .task_evidence()
            .validate(&verification_evidence.id)
            .expect("validate verification evidence");
        let completed = database
            .complete_expert_workflow_stage(
                &workflow_run_id,
                "verify",
                &json!({ "tests": "passed" }),
                Some(&json!({ "status": "verified" })),
                false,
            )
            .expect("complete workflow");
        assert_eq!(completed.run.status, "completed");
        assert_eq!(completed.run.output, Some(json!({ "status": "verified" })));
        assert!(database
            .active_expert_workflow(&conversation.id)
            .expect("query active workflow")
            .is_none());
        assert_eq!(
            database
                .goals()
                .get(&placeholder.id)
                .expect("load goal")
                .expect("workflow goal")
                .status,
            GoalStatus::Completed
        );
        cleanup(database, path);
    }

    #[test]
    fn retries_only_to_the_declared_bound_then_blocks_the_goal() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("retry"), None, None)
            .expect("create conversation");
        let binding = database
            .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
            .expect("bind workflow expert");
        let owner_run = database
            .create_run(&conversation.id, "retry workflow", None)
            .expect("create owner run");
        let snapshot = database
            .start_expert_workflow(&workflow_input(
                &conversation.id,
                &binding.id,
                &binding.package_hash,
                false,
                false,
                2,
            ))
            .expect("start workflow");
        let workflow_run_id = snapshot.run.id.clone();
        database
            .start_expert_workflow_stage(&workflow_run_id, "diagnose", &owner_run.run.id)
            .expect("start first attempt");
        let retry = database
            .fail_expert_workflow_stage(&workflow_run_id, "diagnose", "transient")
            .expect("schedule retry");
        assert_eq!(retry.run.status, "running");
        assert_eq!(retry.stages[0].status, "queued");
        assert_eq!(retry.stages[0].attempt, 1);

        database
            .start_expert_workflow_stage(&workflow_run_id, "diagnose", &owner_run.run.id)
            .expect("start second attempt");
        let failed = database
            .fail_expert_workflow_stage(&workflow_run_id, "diagnose", "permanent")
            .expect("exhaust retry budget");
        assert_eq!(failed.run.status, "failed");
        assert_eq!(failed.stages[0].status, "failed");
        assert_eq!(failed.stages[0].attempt, 2);
        assert_eq!(
            database
                .goals()
                .get(&failed.run.goal_id)
                .expect("load goal")
                .expect("workflow goal")
                .status,
            GoalStatus::Blocked
        );
        cleanup(database, path);
    }
}
