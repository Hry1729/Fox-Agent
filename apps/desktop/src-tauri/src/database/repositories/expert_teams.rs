use super::{now_ms, Database};
use crate::database::{ExpertTeamRunRecord, ExpertTeamSnapshot, StartExpertTeamInput};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

impl Database {
    pub fn start_expert_team(
        &self,
        input: &StartExpertTeamInput,
    ) -> Result<ExpertTeamSnapshot, String> {
        if let Some(existing) = self.active_expert_team(&input.conversation_id)? {
            if existing.run.parent_run_id == input.parent_run_id
                && existing.run.package_hash == input.package_hash
                && existing.run.team_id == input.team_id
            {
                return Ok(existing);
            }
            return Err("team.already_active".to_owned());
        }
        let now = now_ms();
        let team_run_id = Uuid::new_v4().to_string();
        let team_json = serde_json::to_string(&input.team).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (run_conversation_id, run_status, run_kind, run_depth): (
                String,
                String,
                String,
                i64,
            ) = transaction.query_row(
                "SELECT conversation_id, status, run_kind, depth FROM runs WHERE id = ?1",
                [&input.parent_run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            if run_conversation_id != input.conversation_id
                || !matches!(run_status.as_str(), "queued" | "running")
                || run_kind != "primary"
                || run_depth != 0
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let binding_valid: bool = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM conversation_expert_bindings
                    WHERE id = ?1 AND conversation_id = ?2 AND expert_id = ?3
                      AND package_hash = ?4 AND state = 'active'
                 )",
                params![
                    input.expert_binding_id,
                    input.conversation_id,
                    input.expert_id,
                    input.package_hash,
                ],
                |row| row.get(0),
            )?;
            if !binding_valid {
                return Err(rusqlite::Error::InvalidQuery);
            }
            transaction.execute(
                "INSERT INTO expert_team_runs(
                    id, conversation_id, expert_binding_id, expert_id, package_hash,
                    team_id, team_version, team_json, parent_run_id, status,
                    objective, context, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'running', ?10, ?11, ?12, ?12)",
                params![
                    team_run_id,
                    input.conversation_id,
                    input.expert_binding_id,
                    input.expert_id,
                    input.package_hash,
                    input.team_id,
                    input.team_version,
                    team_json,
                    input.parent_run_id,
                    input.objective,
                    input.context,
                    now,
                ],
            )?;
            transaction.commit()
        })?;
        self.get_expert_team(&team_run_id)?
            .ok_or_else(|| "team.start_not_persisted".to_owned())
    }

    pub fn active_expert_team(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ExpertTeamSnapshot>, String> {
        let id = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id FROM expert_team_runs
                     WHERE conversation_id = ?1 AND status = 'running'
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    [conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })?;
        id.map(|id| self.get_expert_team(&id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn latest_expert_team(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ExpertTeamSnapshot>, String> {
        let id = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id FROM expert_team_runs WHERE conversation_id = ?1
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    [conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })?;
        id.map(|id| self.get_expert_team(&id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn get_expert_team(&self, team_run_id: &str) -> Result<Option<ExpertTeamSnapshot>, String> {
        let run = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, expert_binding_id, expert_id, package_hash,
                            team_id, team_version, team_json, parent_run_id, status,
                            objective, context, result_json, error_message,
                            created_at, updated_at, completed_at
                     FROM expert_team_runs WHERE id = ?1",
                    [team_run_id],
                    map_team_run,
                )
                .optional()
        })?;
        let Some(run) = run else {
            return Ok(None);
        };
        let members = self.child_runs_for_team(team_run_id)?;
        Ok(Some(ExpertTeamSnapshot { run, members }))
    }

    pub fn finalize_expert_team(&self, team_run_id: &str) -> Result<ExpertTeamSnapshot, String> {
        let snapshot = self
            .get_expert_team(team_run_id)?
            .ok_or_else(|| "team.not_found".to_owned())?;
        if snapshot.run.status != "running" {
            return Ok(snapshot);
        }
        let expected_members = snapshot
            .run
            .team
            .get("members")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if snapshot.members.len() < expected_members
            || snapshot
                .members
                .iter()
                .any(|member| matches!(member.status.as_str(), "queued" | "running" | "cancelling"))
        {
            return Ok(snapshot);
        }
        let completed = snapshot
            .members
            .iter()
            .all(|member| member.status == "completed");
        let status = if completed { "completed" } else { "failed" };
        let error_message = (!completed)
            .then_some("one or more expert team members did not complete successfully".to_owned());
        let result = json!({
            "members": snapshot.members.iter().map(|member| json!({
                "memberId": member.team_member_id,
                "childRunId": member.child_run_id,
                "status": member.status,
                "result": member.result_text,
                "inputTokens": member.input_tokens,
                "outputTokens": member.output_tokens,
                "totalTokens": member.total_tokens,
                "toolCallCount": member.tool_call_count,
                "errorCode": member.error_code,
                "errorMessage": member.error_message,
            })).collect::<Vec<_>>()
        });
        let result_json = serde_json::to_string(&result).map_err(|error| error.to_string())?;
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE expert_team_runs
                 SET status = ?2, result_json = ?3, error_message = ?4,
                     updated_at = ?5, completed_at = ?5
                 WHERE id = ?1 AND status = 'running'",
                params![team_run_id, status, result_json, error_message, now],
            )?;
            Ok(())
        })?;
        self.get_expert_team(team_run_id)?
            .ok_or_else(|| "team.not_found".to_owned())
    }

    pub fn cancel_expert_team(
        &self,
        team_run_id: &str,
        reason: &str,
    ) -> Result<ExpertTeamSnapshot, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE expert_team_runs
                 SET status = 'cancelled', error_message = ?2, updated_at = ?3, completed_at = ?3
                 WHERE id = ?1 AND status = 'running'",
                params![team_run_id, reason, now],
            )?;
            Ok(())
        })?;
        self.get_expert_team(team_run_id)?
            .ok_or_else(|| "team.not_found".to_owned())
    }

    pub(crate) fn cancel_expert_teams_for_parent(
        &self,
        parent_run_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE expert_team_runs
                 SET status = 'cancelled', error_message = ?2, updated_at = ?3, completed_at = ?3
                 WHERE parent_run_id = ?1 AND status = 'running'",
                params![parent_run_id, reason, now],
            )?;
            Ok(())
        })
    }
}

fn map_team_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExpertTeamRunRecord> {
    let team_json: String = row.get(7)?;
    let result_json: Option<String> = row.get(12)?;
    let parse = |value: String| {
        serde_json::from_str(&value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                value.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    };
    Ok(ExpertTeamRunRecord {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        expert_binding_id: row.get(2)?,
        expert_id: row.get(3)?,
        package_hash: row.get(4)?,
        team_id: row.get(5)?,
        team_version: row.get(6)?,
        team: parse(team_json)?,
        parent_run_id: row.get(8)?,
        status: row.get(9)?,
        objective: row.get(10)?,
        context: row.get(11)?,
        result: result_json.map(parse).transpose()?,
        error_message: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        completed_at: row.get(16)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{ChildRunBudget, CreateChildRunInput};
    use std::path::PathBuf;

    fn cleanup(database: Database, path: PathBuf) {
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    fn team_value() -> Value {
        json!({
            "schemaVersion": 1,
            "id": "delivery.team",
            "version": "1.0.0",
            "title": "Delivery team",
            "description": "Implement and review one delivery",
            "strategy": "supervisor",
            "members": [
                { "id": "implementer", "agentId": "fox-general" },
                { "id": "reviewer", "agentId": "fox-general" }
            ]
        })
    }

    fn budget() -> ChildRunBudget {
        ChildRunBudget {
            max_duration_ms: 60_000,
            max_total_tokens: 8_000,
            max_output_tokens: 2_000,
            max_tool_calls: 8,
        }
    }

    #[test]
    fn persists_serial_members_scopes_and_aggregate_result() {
        let path = std::env::temp_dir().join(format!("fox-expert-team-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open team database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("team"), None, None)
            .expect("create conversation");
        let binding = database
            .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
            .expect("bind expert");
        let parent = database
            .create_run(&conversation.id, "coordinate team", None)
            .expect("create parent Run");
        database
            .apply_runtime_event(&parent.run.id, 1, &json!({ "type": "run.started" }))
            .expect("start parent Run");
        let team = database
            .start_expert_team(&StartExpertTeamInput {
                conversation_id: conversation.id.clone(),
                expert_binding_id: binding.id,
                expert_id: binding.expert_id,
                package_hash: binding.package_hash,
                team_id: "delivery.team".to_owned(),
                team_version: "1.0.0".to_owned(),
                team: team_value(),
                parent_run_id: parent.run.id.clone(),
                objective: "Deliver verified code".to_owned(),
                context: "Only the explicit project facts".to_owned(),
            })
            .expect("start team");
        let allowed = vec!["read".to_owned(), "grep".to_owned()];
        let (_, first, _) = database
            .create_child_run(CreateChildRunInput {
                parent_run_id: &parent.run.id,
                tool_call_id: "team-implementer",
                worker_agent_id: "fox-general",
                objective: "Implement",
                context: "isolated implementer context",
                budget: &budget(),
                team_run_id: Some(&team.run.id),
                team_member_id: Some("implementer"),
                allowed_tools: Some(&allowed),
            })
            .expect("create first member");
        assert_eq!(first.allowed_tools, Some(allowed.clone()));
        let serial_error = database
            .create_child_run(CreateChildRunInput {
                parent_run_id: &parent.run.id,
                tool_call_id: "team-reviewer-too-early",
                worker_agent_id: "fox-general",
                objective: "Review",
                context: "isolated reviewer context",
                budget: &budget(),
                team_run_id: Some(&team.run.id),
                team_member_id: Some("reviewer"),
                allowed_tools: Some(&allowed),
            })
            .expect_err("active member must enforce serial dispatch");
        assert!(!serial_error.is_empty());
        database
            .apply_runtime_event(&first.child_run_id, 1, &json!({ "type": "run.completed" }))
            .expect("complete first member");
        let (_, second, _) = database
            .create_child_run(CreateChildRunInput {
                parent_run_id: &parent.run.id,
                tool_call_id: "team-reviewer",
                worker_agent_id: "fox-general",
                objective: "Review",
                context: "isolated reviewer context",
                budget: &budget(),
                team_run_id: Some(&team.run.id),
                team_member_id: Some("reviewer"),
                allowed_tools: Some(&allowed),
            })
            .expect("create second member");
        database
            .apply_runtime_event(&second.child_run_id, 1, &json!({ "type": "run.completed" }))
            .expect("complete second member");
        let completed = database
            .finalize_expert_team(&team.run.id)
            .expect("finalize team");
        assert_eq!(completed.run.status, "completed");
        assert_eq!(completed.members.len(), 2);
        assert_eq!(
            completed.run.result.as_ref().unwrap()["members"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        drop(database);
        let database = Database::open(path.clone()).expect("reopen team database");
        let restored = database
            .latest_expert_team(&conversation.id)
            .expect("load team")
            .expect("persisted team");
        assert_eq!(restored.run.status, "completed");
        assert_eq!(
            restored.members[0].team_member_id.as_deref(),
            Some("implementer")
        );
        cleanup(database, path);
    }
}
