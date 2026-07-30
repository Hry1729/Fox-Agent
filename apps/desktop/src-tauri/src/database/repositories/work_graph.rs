use super::{now_ms, Database};
use crate::database::{
    EvidenceReferenceKind, EvidenceType, EvidenceValidityStatus, GoalRecord, GoalStatus,
    TaskEvidenceRecord, WorkTaskRecord, WorkTaskStatus,
};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::Value;
use std::{fmt, thread, time::Duration};
use uuid::Uuid;

const MAX_BUSY_ATTEMPTS: usize = 5;
const INITIAL_BUSY_DELAY_MS: u64 = 10;
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug)]
pub enum RepositoryError {
    NotFound {
        entity: &'static str,
        id: String,
    },
    InvalidInput(String),
    InvalidTransition {
        entity: &'static str,
        from: String,
        to: String,
    },
    OptimisticLockFailed {
        id: String,
        expected_version: i64,
    },
    ConstraintViolation(String),
    EvidenceRequired {
        task_id: String,
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
            Self::ConstraintViolation(message) => formatter.write_str(message),
            Self::EvidenceRequired { task_id } => {
                write!(
                    formatter,
                    "work task '{task_id}' requires evidence before completion"
                )
            }
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
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let prepared = inputs
            .into_iter()
            .map(|input| {
                validate_non_empty("goal_id", &input.goal_id)?;
                validate_non_empty("title", &input.title)?;
                if input.ordinal < 0 {
                    return Err(RepositoryError::InvalidInput(
                        "task ordinal cannot be negative".to_owned(),
                    ));
                }
                Ok((
                    input
                        .id
                        .clone()
                        .unwrap_or_else(|| Uuid::new_v4().to_string()),
                    input,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let now = timestamp();
        with_write_transaction(&self.database, |transaction| {
            let mut created = Vec::with_capacity(prepared.len());
            for (id, input) in &prepared {
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
            );
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

fn with_write_transaction<T>(
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

fn validate_reference(
    connection: &Connection,
    task_id: &str,
    evidence_type: &EvidenceType,
    ref_kind: &EvidenceReferenceKind,
    ref_id: &str,
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
            if matches!(status.as_str(), "pending" | "running") {
                return Err(RepositoryError::InvalidReference(format!(
                    "tool call '{ref_id}' is not terminal"
                )));
            }
        }
        EvidenceReferenceKind::Artifact => {
            let target: Option<(String, String)> = connection
                .query_row(
                    "SELECT conversation_id, status FROM artifacts WHERE id = ?1",
                    [ref_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(RepositoryError::database)?;
            let (target_conversation, status) = target.ok_or_else(|| {
                RepositoryError::MissingReference(format!("artifact '{ref_id}' was not found"))
            })?;
            ensure_same_conversation(&conversation_id, &target_conversation)?;
            if status != "ready" {
                return Err(RepositoryError::InvalidReference(format!(
                    "artifact '{ref_id}' is not accessible"
                )));
            }
        }
        EvidenceReferenceKind::RunEvent => {
            let target: Option<(String, Option<String>)> = connection
                .query_row(
                    "SELECT runs.conversation_id, run_events.span_id
                     FROM run_events JOIN runs ON runs.id = run_events.run_id
                     WHERE run_events.id = ?1",
                    [ref_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(RepositoryError::database)?;
            let (target_conversation, span_id) = target.ok_or_else(|| {
                RepositoryError::MissingReference(format!("run event '{ref_id}' was not found"))
            })?;
            ensure_same_conversation(&conversation_id, &target_conversation)?;
            if matches!(evidence_type, EvidenceType::TraceSpan) && span_id.is_none() {
                return Err(RepositoryError::InvalidReference(format!(
                    "run event '{ref_id}' has no span"
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
        "goals" => "SELECT EXISTS(SELECT 1 FROM goals WHERE id = ?1)",
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

fn load_task(connection: &Connection, id: &str) -> Result<Option<WorkTaskRecord>, RepositoryError> {
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

fn load_evidence(
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
    use std::path::PathBuf;

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

    fn cleanup(database: Database, path: PathBuf) {
        drop(database);
        let _ = std::fs::remove_file(path);
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
    fn task_state_machine_enforces_evidence_single_progress_and_retry_attempt() {
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
        database
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
        assert!(matches!(
            tasks.skip(&retried.id),
            Err(RepositoryError::InvalidTransition { .. })
        ));
        cleanup(database, path);
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
        cleanup(database, path);
    }

    #[test]
    fn evidence_accepts_every_supported_local_reference_kind() {
        let (database, path) = test_database();
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
                               'report.json', 'ready', 1, 1)",
                    params![conversation_id, run.id],
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
}
