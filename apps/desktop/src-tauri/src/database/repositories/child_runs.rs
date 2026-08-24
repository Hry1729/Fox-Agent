use super::{now_ms, observability, Database};
use crate::database::{
    ChildAgentSummary, ChildRunBudget, ChildRunRecord, MessageRecord, RunRecord, StartRunResult,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use uuid::Uuid;

pub(crate) const MAX_CHILD_DEPTH: i64 = 1;
pub(crate) const MAX_CHILDREN_PER_PARENT: i64 = 8;
pub(crate) const MAX_ACTIVE_CHILDREN_PER_ROOT: i64 = 3;

pub(crate) struct CreateChildRunInput<'a> {
    pub parent_run_id: &'a str,
    pub tool_call_id: &'a str,
    pub worker_agent_id: &'a str,
    pub objective: &'a str,
    pub context: &'a str,
    pub budget: &'a ChildRunBudget,
    pub team_run_id: Option<&'a str>,
    pub team_member_id: Option<&'a str>,
    pub allowed_tools: Option<&'a [String]>,
}

impl Database {
    pub fn list_child_agents(&self) -> Result<Vec<ChildAgentSummary>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, name, description, category, agent_kind
                 FROM agents
                 WHERE runtime_type = 'pi'
                   AND (agent_kind = 'assistant' OR agent_kind = 'expert' OR agent_kind = 'worker')
                 ORDER BY CASE id WHEN 'fox-general' THEN 0 ELSE 1 END, name, id",
            )?;
            let records = statement
                .query_map([], |row| {
                    Ok(ChildAgentSummary {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        category: row.get(3)?,
                        agent_kind: row.get(4)?,
                    })
                })?
                .collect();
            records
        })
    }

    pub(crate) fn create_child_run(
        &self,
        input: CreateChildRunInput<'_>,
    ) -> Result<(StartRunResult, ChildRunRecord, bool), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if let Some(existing) = query_child_run_by_tool_call(
                &transaction,
                input.parent_run_id,
                input.tool_call_id,
            )? {
                let started = query_child_start_result(&transaction, &existing.child_run_id)?;
                return Ok((started, existing, false));
            }

            let (
                parent_conversation_id,
                project_root,
                project_id,
                lineage_root_id,
                model,
                root_run_id,
                parent_depth,
                parent_status,
            ): (
                String,
                Option<String>,
                Option<String>,
                String,
                String,
                String,
                i64,
                String,
            ) = transaction.query_row(
                "SELECT c.id, c.project_root, c.project_id, COALESCE(c.lineage_root_id, c.id),
                        r.model, COALESCE(r.root_run_id, r.id), r.depth, r.status
                 FROM runs r JOIN conversations c ON c.id = r.conversation_id
                 WHERE r.id = ?1",
                [input.parent_run_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )?;
            if !matches!(parent_status.as_str(), "queued" | "running") {
                return Err(rusqlite::Error::InvalidQuery);
            }
            if parent_depth >= MAX_CHILD_DEPTH {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let worker_name: String = transaction.query_row(
                "SELECT name FROM agents
                 WHERE id = ?1 AND runtime_type = 'pi'
                   AND (agent_kind = 'assistant' OR agent_kind = 'expert' OR agent_kind = 'worker')",
                [input.worker_agent_id],
                |row| row.get(0),
            )?;
            let child_count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM child_run_delegations WHERE parent_run_id = ?1",
                [input.parent_run_id],
                |row| row.get(0),
            )?;
            if child_count >= MAX_CHILDREN_PER_PARENT {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let active_count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM runs
                 WHERE root_run_id = ?1 AND run_kind = 'child'
                   AND status IN ('queued', 'running', 'cancelling')",
                [&root_run_id],
                |row| row.get(0),
            )?;
            if active_count >= MAX_ACTIVE_CHILDREN_PER_ROOT {
                return Err(rusqlite::Error::InvalidQuery);
            }
            if let (Some(team_run_id), Some(team_member_id)) =
                (input.team_run_id, input.team_member_id)
            {
                let (team_parent_run_id, team_status, team_json): (String, String, String) =
                    transaction.query_row(
                        "SELECT parent_run_id, status, team_json FROM expert_team_runs WHERE id = ?1",
                        [team_run_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )?;
                if team_parent_run_id != input.parent_run_id || team_status != "running" {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let team: Value = serde_json::from_str(&team_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        team_json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                let member_exists = team
                    .get("members")
                    .and_then(Value::as_array)
                    .is_some_and(|members| {
                        members.iter().any(|member| {
                            member.get("id").and_then(Value::as_str) == Some(team_member_id)
                                && member.get("agentId").and_then(Value::as_str)
                                    == Some(input.worker_agent_id)
                        })
                    });
                let active_team_member_count: i64 = transaction.query_row(
                    "SELECT COUNT(*) FROM child_run_delegations
                     WHERE team_run_id = ?1 AND status IN ('queued', 'running')",
                    [team_run_id],
                    |row| row.get(0),
                )?;
                if !member_exists || active_team_member_count > 0 {
                    return Err(rusqlite::Error::InvalidQuery);
                }
            } else if input.team_run_id.is_some() || input.team_member_id.is_some() {
                return Err(rusqlite::Error::InvalidQuery);
            }

            let delegation_id = Uuid::new_v4().to_string();
            let child_conversation_id = Uuid::new_v4().to_string();
            let child_run_id = Uuid::new_v4().to_string();
            let message_id = Uuid::new_v4().to_string();
            let title = super::truncate_title(input.objective);
            let runtime_text = child_runtime_text(input.objective, input.context);
            transaction.execute(
                "INSERT INTO conversations(
                    id, agent_id, title, project_root, project_id, status, created_at, updated_at,
                    last_message_at, parent_conversation_id, lineage_root_id, conversation_kind
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6, ?6, ?7, ?8, 'child')",
                params![
                    child_conversation_id,
                    input.worker_agent_id,
                    title,
                    project_root,
                    project_id,
                    now,
                    parent_conversation_id,
                    lineage_root_id,
                ],
            )?;
            let budget_json = serde_json::to_string(input.budget)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            let allowed_tools_json = input
                .allowed_tools
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            transaction.execute(
                "INSERT INTO runs(
                    id, conversation_id, status, model, created_at, parent_run_id, root_run_id,
                    run_kind, depth, budget_json
                 ) VALUES (?1, ?2, 'queued', ?3, ?4, ?5, ?6, 'child', ?7, ?8)",
                params![
                    child_run_id,
                    child_conversation_id,
                    model,
                    now,
                    input.parent_run_id,
                    root_run_id,
                    parent_depth + 1,
                    budget_json,
                ],
            )?;
            let (trace_id, root_span_id) = observability::initialize_child_run_trace(
                &transaction,
                &child_run_id,
                input.parent_run_id,
                input.tool_call_id,
                input.worker_agent_id,
                &worker_name,
                now,
            )?;
            transaction.execute(
                "INSERT INTO messages(
                    id, conversation_id, run_id, role, kind, content, status, ordinal,
                    created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'user', 'text', ?4, 'completed', 1, ?5, ?5)",
                params![message_id, child_conversation_id, child_run_id, runtime_text, now],
            )?;
            transaction.execute(
                "INSERT INTO child_run_delegations(
                    id, parent_run_id, child_run_id, child_conversation_id, tool_call_id,
                    worker_agent_id, objective, context, status, max_duration_ms,
                    max_total_tokens, max_output_tokens, max_tool_calls, created_at,
                    team_run_id, team_member_id, allowed_tools_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    delegation_id,
                    input.parent_run_id,
                    child_run_id,
                    child_conversation_id,
                    input.tool_call_id,
                    input.worker_agent_id,
                    input.objective,
                    input.context,
                    input.budget.max_duration_ms,
                    input.budget.max_total_tokens,
                    input.budget.max_output_tokens,
                    input.budget.max_tool_calls,
                    now,
                    input.team_run_id,
                    input.team_member_id,
                    allowed_tools_json,
                ],
            )?;
            let started = StartRunResult {
                run: RunRecord {
                    id: child_run_id.clone(),
                    conversation_id: child_conversation_id.clone(),
                    runtime_session_id: None,
                    status: "queued".to_owned(),
                    model,
                    started_at: None,
                    finished_at: None,
                    error_code: None,
                    error_message: None,
                    last_seq: 0,
                    trace_id: Some(trace_id),
                    root_span_id: Some(root_span_id),
                },
                user_message: MessageRecord {
                    id: message_id,
                    conversation_id: child_conversation_id,
                    run_id: Some(child_run_id.clone()),
                    role: "user".to_owned(),
                    kind: "text".to_owned(),
                    content: runtime_text,
                    status: "completed".to_owned(),
                    ordinal: 1,
                    created_at: now,
                    updated_at: now,
                },
                attachments: Vec::new(),
            };
            let child = query_child_run(&transaction, &child_run_id)?;
            transaction.commit()?;
            Ok((started, child, true))
        })
    }

    pub fn child_runs_for_parent(
        &self,
        parent_run_id: &str,
    ) -> Result<Vec<ChildRunRecord>, String> {
        self.with_connection(|connection| {
            query_child_runs(connection, "d.parent_run_id = ?1", parent_run_id)
        })
    }

    pub fn child_runs_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ChildRunRecord>, String> {
        self.with_connection(|connection| {
            query_child_runs(
                connection,
                "EXISTS(SELECT 1 FROM runs parent WHERE parent.id = d.parent_run_id AND parent.conversation_id = ?1)",
                conversation_id,
            )
        })
    }

    pub(crate) fn child_runs_for_team(
        &self,
        team_run_id: &str,
    ) -> Result<Vec<ChildRunRecord>, String> {
        self.with_connection(|connection| {
            query_child_runs(connection, "d.team_run_id = ?1", team_run_id)
        })
    }

    pub fn child_run(&self, child_run_id: &str) -> Result<Option<ChildRunRecord>, String> {
        self.with_connection(|connection| query_child_run_optional(connection, child_run_id))
    }

    pub(crate) fn child_run_approvals_for_conversation(
        &self,
        parent_conversation_id: &str,
    ) -> Result<Vec<crate::database::ApprovalRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT a.id, a.tool_call_id, t.run_id, t.conversation_id, t.tool_name,
                        a.status, a.requested_action, a.request_json, a.decision_json,
                        a.requested_at, a.resolved_at
                 FROM approvals a
                 JOIN tool_calls t ON t.id = a.tool_call_id
                 JOIN conversations child ON child.id = t.conversation_id
                 WHERE child.conversation_kind = 'child'
                   AND child.parent_conversation_id = ?1
                 ORDER BY a.requested_at ASC, a.id ASC",
            )?;
            let records = statement
                .query_map([parent_conversation_id], super::map_approval)?
                .collect();
            records
        })
    }

    pub(crate) fn child_run_status(&self, child_run_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT status FROM runs WHERE id = ?1 AND run_kind = 'child'",
                    [child_run_id],
                    |row| row.get(0),
                )
                .optional()
        })
    }

    pub(crate) fn active_child_run_ids(&self, run_id: &str) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "WITH RECURSIVE descendants(id, depth, created_at, status) AS (
                     SELECT id, depth, created_at, status FROM runs WHERE parent_run_id = ?1
                     UNION ALL
                     SELECT child.id, child.depth, child.created_at, child.status
                     FROM runs child JOIN descendants parent ON child.parent_run_id = parent.id
                 )
                 SELECT id FROM descendants
                 WHERE status IN ('queued', 'running', 'cancelling')
                 ORDER BY depth DESC, created_at DESC, id DESC",
            )?;
            let run_ids = statement.query_map([run_id], |row| row.get(0))?.collect();
            run_ids
        })
    }

    pub(crate) fn enforce_child_tool_budget(&self, run_id: &str) -> Result<(), String> {
        let budget = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT d.max_tool_calls, d.max_total_tokens,
                            (SELECT COUNT(*) FROM tool_calls t WHERE t.run_id = d.child_run_id),
                            d.total_tokens
                     FROM child_run_delegations d WHERE d.child_run_id = ?1",
                    [run_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .optional()
        })?;
        let Some((max_tools, max_tokens, used_tools, used_tokens)) = budget else {
            return Ok(());
        };
        if used_tools >= max_tools {
            return Err(format!(
                "child_run.tool_budget_exceeded: Child Run used {used_tools} of {max_tools} allowed tool calls"
            ));
        }
        if used_tokens >= max_tokens {
            return Err(format!(
                "child_run.token_budget_exceeded: Child Run used {used_tokens} of {max_tokens} allowed tokens"
            ));
        }
        Ok(())
    }

    pub(crate) fn child_budget_exceeded(&self, run_id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT total_tokens >= max_total_tokens
                 FROM child_run_delegations WHERE child_run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
        })
    }

    pub(crate) fn sync_child_run_terminal(&self, run_id: &str) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| sync_child_terminal(connection, run_id, now))
    }
}

fn child_runtime_text(objective: &str, context: &str) -> String {
    if context.trim().is_empty() {
        return objective.trim().to_owned();
    }
    format!(
        "# Delegated objective\n{}\n\n# Explicit parent context\n{}",
        objective.trim(),
        context.trim()
    )
}

pub(super) fn project_child_run_event(
    transaction: &Transaction<'_>,
    run_id: &str,
    event_type: &str,
    payload: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    match event_type {
        "run.started" => {
            transaction.execute(
                "UPDATE child_run_delegations
                 SET status = 'running', started_at = COALESCE(started_at, ?2)
                 WHERE child_run_id = ?1",
                params![run_id, now],
            )?;
        }
        "usage.updated" => {
            let input_tokens = payload
                .get("inputTokens")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0);
            let output_tokens = payload
                .get("outputTokens")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0);
            let total_tokens = payload
                .get("totalTokens")
                .and_then(Value::as_i64)
                .unwrap_or(input_tokens.saturating_add(output_tokens))
                .max(0);
            transaction.execute(
                "UPDATE child_run_delegations
                 SET input_tokens = ?2, output_tokens = ?3, total_tokens = ?4
                 WHERE child_run_id = ?1",
                params![run_id, input_tokens, output_tokens, total_tokens],
            )?;
        }
        "run.completed" | "run.cancelled" | "run.failed" | "run.interrupted" => {
            sync_child_terminal(transaction, run_id, now)?;
        }
        _ => {}
    }
    Ok(())
}

fn sync_child_terminal(
    connection: &rusqlite::Connection,
    run_id: &str,
    now: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE child_run_delegations
         SET status = (SELECT status FROM runs WHERE id = ?1),
             result_text = NULLIF((
                 SELECT substr(content, 1, 32000) FROM messages
                 WHERE run_id = ?1 AND role = 'assistant'
                 ORDER BY ordinal DESC LIMIT 1
             ), ''),
             tool_call_count = (SELECT COUNT(*) FROM tool_calls WHERE run_id = ?1),
             error_code = (SELECT error_code FROM runs WHERE id = ?1),
             error_message = (SELECT error_message FROM runs WHERE id = ?1),
             started_at = COALESCE(started_at, (SELECT started_at FROM runs WHERE id = ?1)),
             finished_at = COALESCE((SELECT finished_at FROM runs WHERE id = ?1), ?2)
         WHERE child_run_id = ?1",
        params![run_id, now],
    )?;
    Ok(())
}

fn child_run_select() -> &'static str {
    "SELECT d.id, d.parent_run_id, d.child_run_id, d.child_conversation_id,
            d.worker_agent_id, a.name, d.objective, d.context,
            d.team_run_id, d.team_member_id, d.allowed_tools_json, d.status, r.depth,
            d.max_duration_ms, d.max_total_tokens, d.max_output_tokens, d.max_tool_calls,
            d.result_text, d.input_tokens, d.output_tokens, d.total_tokens,
            d.tool_call_count, d.error_code, d.error_message, d.created_at,
            d.started_at, d.finished_at
     FROM child_run_delegations d
     JOIN runs r ON r.id = d.child_run_id
     JOIN agents a ON a.id = d.worker_agent_id"
}

fn map_child_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChildRunRecord> {
    let allowed_tools_json = row.get::<_, Option<String>>(10)?;
    let allowed_tools = allowed_tools_json
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    value.len(),
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    Ok(ChildRunRecord {
        id: row.get(0)?,
        parent_run_id: row.get(1)?,
        child_run_id: row.get(2)?,
        child_conversation_id: row.get(3)?,
        worker_agent_id: row.get(4)?,
        worker_agent_name: row.get(5)?,
        objective: row.get(6)?,
        context: row.get(7)?,
        team_run_id: row.get(8)?,
        team_member_id: row.get(9)?,
        allowed_tools,
        status: row.get(11)?,
        depth: row.get(12)?,
        budget: ChildRunBudget {
            max_duration_ms: row.get(13)?,
            max_total_tokens: row.get(14)?,
            max_output_tokens: row.get(15)?,
            max_tool_calls: row.get(16)?,
        },
        result_text: row.get(17)?,
        input_tokens: row.get(18)?,
        output_tokens: row.get(19)?,
        total_tokens: row.get(20)?,
        tool_call_count: row.get(21)?,
        error_code: row.get(22)?,
        error_message: row.get(23)?,
        created_at: row.get(24)?,
        started_at: row.get(25)?,
        finished_at: row.get(26)?,
    })
}

fn query_child_run(
    connection: &rusqlite::Connection,
    child_run_id: &str,
) -> rusqlite::Result<ChildRunRecord> {
    connection.query_row(
        &format!("{} WHERE d.child_run_id = ?1", child_run_select()),
        [child_run_id],
        map_child_run,
    )
}

fn query_child_run_optional(
    connection: &rusqlite::Connection,
    child_run_id: &str,
) -> rusqlite::Result<Option<ChildRunRecord>> {
    connection
        .query_row(
            &format!("{} WHERE d.child_run_id = ?1", child_run_select()),
            [child_run_id],
            map_child_run,
        )
        .optional()
}

fn query_child_run_by_tool_call(
    connection: &rusqlite::Connection,
    parent_run_id: &str,
    tool_call_id: &str,
) -> rusqlite::Result<Option<ChildRunRecord>> {
    connection
        .query_row(
            &format!(
                "{} WHERE d.parent_run_id = ?1 AND d.tool_call_id = ?2",
                child_run_select()
            ),
            params![parent_run_id, tool_call_id],
            map_child_run,
        )
        .optional()
}

fn query_child_runs(
    connection: &rusqlite::Connection,
    predicate: &str,
    value: &str,
) -> rusqlite::Result<Vec<ChildRunRecord>> {
    let query = format!(
        "{} WHERE {} ORDER BY d.created_at ASC, d.id ASC",
        child_run_select(),
        predicate
    );
    let mut statement = connection.prepare(&query)?;
    let records = statement
        .query_map([value], map_child_run)?
        .collect::<Result<Vec<_>, _>>();
    records
}

fn query_child_start_result(
    connection: &rusqlite::Connection,
    run_id: &str,
) -> rusqlite::Result<StartRunResult> {
    let run = connection.query_row(
        "SELECT id, conversation_id, runtime_session_id, status, model, started_at,
                finished_at, error_code, error_message, last_seq, trace_id, root_span_id
         FROM runs WHERE id = ?1",
        [run_id],
        |row| {
            Ok(RunRecord {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                runtime_session_id: row.get(2)?,
                status: row.get(3)?,
                model: row.get(4)?,
                started_at: row.get(5)?,
                finished_at: row.get(6)?,
                error_code: row.get(7)?,
                error_message: row.get(8)?,
                last_seq: row.get(9)?,
                trace_id: row.get(10)?,
                root_span_id: row.get(11)?,
            })
        },
    )?;
    let user_message = connection.query_row(
        "SELECT id, conversation_id, run_id, role, kind, content, status, ordinal,
                created_at, updated_at
         FROM messages WHERE run_id = ?1 AND role = 'user' ORDER BY ordinal LIMIT 1",
        [run_id],
        |row| {
            Ok(MessageRecord {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                run_id: row.get(2)?,
                role: row.get(3)?,
                kind: row.get(4)?,
                content: row.get(5)?,
                status: row.get(6)?,
                ordinal: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        },
    )?;
    Ok(StartRunResult {
        run,
        user_message,
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn test_database() -> (Database, PathBuf, StartRunResult) {
        let path = std::env::temp_dir().join(format!("fox-child-runs-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open test database");
        let conversation = database
            .create_conversation("fox-general", Some("parent"), Some("D:/workspace"), None)
            .expect("create parent conversation");
        let started = database
            .create_run(&conversation.id, "Coordinate independent research", None)
            .expect("create parent run");
        database
            .apply_runtime_event(&started.run.id, 1, &json!({ "type": "run.started" }))
            .expect("start parent run");
        (database, path, started)
    }

    fn default_budget() -> ChildRunBudget {
        ChildRunBudget {
            max_duration_ms: 30_000,
            max_total_tokens: 4_000,
            max_output_tokens: 1_000,
            max_tool_calls: 4,
        }
    }

    fn create_child(
        database: &Database,
        parent_run_id: &str,
        tool_call_id: &str,
    ) -> (StartRunResult, ChildRunRecord, bool) {
        database
            .create_host_tool_call(
                parent_run_id,
                tool_call_id,
                "child_run_start",
                &json!({ "objective": "Inspect one isolated concern" }),
                "running",
                false,
            )
            .expect("create delegation tool call");
        database
            .create_child_run(CreateChildRunInput {
                parent_run_id,
                tool_call_id,
                worker_agent_id: "fox-general",
                objective: "Inspect one isolated concern",
                context: "Only use this explicit parent context.",
                budget: &default_budget(),
                team_run_id: None,
                team_member_id: None,
                allowed_tools: None,
            })
            .expect("create child run")
    }

    #[test]
    fn child_run_is_isolated_idempotent_and_linked_to_parent_trace() {
        let (database, path, parent) = test_database();
        database
            .record_external_event(
                &parent.run.id,
                &json!({
                    "type": "tool.started",
                    "toolCallId": "delegate-1",
                    "tool": "child_run_start",
                    "input": { "objective": "Inspect one isolated concern" }
                }),
            )
            .expect("project runtime delegation span");
        let (started, child, created) = create_child(&database, &parent.run.id, "delegate-1");
        assert!(created);
        assert_eq!(child.parent_run_id, parent.run.id);
        assert_eq!(child.depth, 1);
        assert_eq!(child.status, "queued");
        assert_ne!(started.run.conversation_id, parent.run.conversation_id);
        assert!(started
            .user_message
            .content
            .contains("# Delegated objective"));
        assert!(started
            .user_message
            .content
            .contains("# Explicit parent context"));

        let (_, duplicate, duplicate_created) =
            create_child(&database, &parent.run.id, "delegate-1");
        assert!(!duplicate_created);
        assert_eq!(duplicate.child_run_id, child.child_run_id);
        assert_eq!(
            database
                .child_runs_for_parent(&parent.run.id)
                .expect("list child runs")
                .len(),
            1
        );
        assert_eq!(
            database
                .list_conversations()
                .expect("list conversations")
                .len(),
            1,
            "hidden child conversations must not enter the primary inbox"
        );
        let protected_tool = database
            .create_host_tool_call(
                &child.child_run_id,
                "child-protected-tool",
                "run_command",
                &json!({ "program": "cargo", "args": ["check"] }),
                "pending",
                true,
            )
            .expect("create protected child tool call");
        let approval = database
            .create_approval(
                &protected_tool.id,
                "Run a bounded check",
                &json!({ "program": "cargo" }),
            )
            .expect("create child approval");
        assert!(database
            .load_conversation(&parent.run.conversation_id)
            .expect("reload parent conversation")
            .approvals
            .iter()
            .any(|item| item.id == approval.id
                && item.conversation_id == child.child_conversation_id));

        database
            .with_connection(|connection| {
                let (parent_trace, delegation_span): (String, String) = connection.query_row(
                    "SELECT r.trace_id, t.span_id
                     FROM runs r JOIN tool_calls t ON t.run_id = r.id
                     WHERE r.id = ?1 AND t.runtime_tool_call_id = 'delegate-1'",
                    [&parent.run.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                let (child_trace, child_root): (String, String) = connection.query_row(
                    "SELECT trace_id, root_span_id FROM runs WHERE id = ?1",
                    [&child.child_run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                let parent_span: String = connection.query_row(
                    "SELECT parent_span_id FROM trace_spans WHERE id = ?1",
                    [&child_root],
                    |row| row.get(0),
                )?;
                assert_eq!(child_trace, parent_trace);
                assert_eq!(parent_span, delegation_span);
                Ok(())
            })
            .expect("verify trace linkage");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn child_run_projects_bounded_usage_and_terminal_result() {
        let (database, path, parent) = test_database();
        let (_, child, _) = create_child(&database, &parent.run.id, "delegate-result");
        let run_id = &child.child_run_id;
        for (seq, event) in [
            (1, json!({ "type": "run.started" })),
            (2, json!({ "type": "message.started" })),
            (
                3,
                json!({ "type": "message.delta", "delta": "Independent finding" }),
            ),
            (4, json!({ "type": "message.completed" })),
            (
                5,
                json!({
                    "type": "usage.updated",
                    "inputTokens": 120,
                    "outputTokens": 30,
                    "totalTokens": 150
                }),
            ),
            (6, json!({ "type": "run.completed" })),
        ] {
            assert!(database
                .apply_runtime_event(run_id, seq, &event)
                .expect("apply child event"));
        }

        let projected = database
            .child_run(run_id)
            .expect("load child run")
            .expect("child run exists");
        assert_eq!(projected.status, "completed");
        assert_eq!(
            projected.result_text.as_deref(),
            Some("Independent finding")
        );
        assert_eq!(projected.input_tokens, 120);
        assert_eq!(projected.output_tokens, 30);
        assert_eq!(projected.total_tokens, 150);
        assert!(projected.finished_at.is_some());
        assert!(database
            .active_child_run_ids(&parent.run.id)
            .unwrap()
            .is_empty());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn child_run_enforces_root_concurrency_and_tool_budget() {
        let (database, path, parent) = test_database();
        let mut first_child_id = String::new();
        for index in 0..MAX_ACTIVE_CHILDREN_PER_ROOT {
            let tool_call_id = format!("delegate-{index}");
            let (_, child, _) = create_child(&database, &parent.run.id, &tool_call_id);
            if first_child_id.is_empty() {
                first_child_id = child.child_run_id;
            }
        }
        assert_eq!(
            database.active_child_run_ids(&parent.run.id).unwrap().len(),
            MAX_ACTIVE_CHILDREN_PER_ROOT as usize
        );
        assert!(
            database
                .active_child_run_ids(&first_child_id)
                .unwrap()
                .is_empty(),
            "a child cancellation must not cascade into sibling Child Runs"
        );
        database
            .create_host_tool_call(
                &parent.run.id,
                "delegate-over-limit",
                "child_run_start",
                &json!({ "objective": "too many" }),
                "running",
                false,
            )
            .unwrap();
        let error = database
            .create_child_run(CreateChildRunInput {
                parent_run_id: &parent.run.id,
                tool_call_id: "delegate-over-limit",
                worker_agent_id: "fox-general",
                objective: "too many",
                context: "",
                budget: &default_budget(),
                team_run_id: None,
                team_member_id: None,
                allowed_tools: None,
            })
            .expect_err("fourth active child must be rejected");
        assert!(!error.is_empty());

        let tool_budget = ChildRunBudget {
            max_tool_calls: 1,
            ..default_budget()
        };
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE child_run_delegations SET max_tool_calls = ?2 WHERE child_run_id = ?1",
                    params![first_child_id, tool_budget.max_tool_calls],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(database.enforce_child_tool_budget(&first_child_id).is_ok());
        database
            .create_host_tool_call(
                &first_child_id,
                "child-tool-1",
                "memory_search",
                &json!({ "query": "bounded" }),
                "running",
                false,
            )
            .unwrap();
        assert!(database
            .enforce_child_tool_budget(&first_child_id)
            .expect_err("tool budget must stop the next call")
            .contains("tool_budget_exceeded"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
