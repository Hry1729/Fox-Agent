use super::{now_ms, Database};
use crate::database::{AcceptanceRecord, PlanRevisionRecord, ReviewFindingRecord};
use rusqlite::{params, OptionalExtension, Row};
use serde_json::Value;
use uuid::Uuid;

impl Database {
    pub fn create_plan_revision(
        &self,
        conversation_id: &str,
        goal_id: &str,
        title: &str,
        summary: &str,
        tasks: Value,
        created_by: &str,
    ) -> Result<PlanRevisionRecord, String> {
        let goal = self
            .goals()
            .get(goal_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "goal was not found".to_owned())?;
        if goal.conversation_id != conversation_id {
            return Err("goal belongs to a different conversation".to_owned());
        }
        let id = Uuid::new_v4().to_string();
        let created_at = now_ms().to_string();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let revision = transaction.query_row(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM plan_revisions WHERE goal_id = ?1",
                [goal_id], |row| row.get::<_, i64>(0),
            )?;
            transaction.execute(
                "UPDATE plan_revisions SET status = 'superseded' WHERE goal_id = ?1 AND status = 'proposed'",
                [goal_id],
            )?;
            transaction.execute(
                "INSERT INTO plan_revisions(id, goal_id, conversation_id, revision, title, summary,
                     tasks_json, status, created_by, created_at, approved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'proposed', ?8, ?9, NULL)",
                params![id, goal_id, conversation_id, revision, title, summary, tasks.to_string(), created_by, created_at],
            )?;
            let record = transaction.query_row("SELECT id, goal_id, conversation_id, revision, title, summary, tasks_json, status, created_by, created_at, approved_at FROM plan_revisions WHERE id = ?1", [&id], plan_revision_from_row)?;
            transaction.commit()?;
            Ok(record)
        })
    }

    pub fn resolve_plan_revision(
        &self,
        conversation_id: &str,
        plan_revision_id: &str,
        decision: &str,
    ) -> Result<PlanRevisionRecord, String> {
        if !["approved", "rejected"].contains(&decision) {
            return Err("plan revision decision must be approved or rejected".to_owned());
        }
        let (plans, _, _) = self.load_a1_snapshot(conversation_id)?;
        let existing = plans
            .iter()
            .find(|plan| plan.id == plan_revision_id)
            .ok_or_else(|| "plan revision was not found in this conversation".to_owned())?;
        if existing.status != "proposed" {
            return Err(format!(
                "plan revision is already resolved with status '{}'",
                existing.status
            ));
        }

        let resolved_at = now_ms().to_string();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = if decision == "approved" {
                transaction.execute(
                    "UPDATE plan_revisions SET status = 'superseded'
                     WHERE goal_id = ?1 AND id <> ?2 AND status IN ('proposed', 'approved')",
                    params![existing.goal_id, plan_revision_id],
                )?;
                transaction.execute(
                    "UPDATE plan_revisions SET status = 'approved', approved_at = ?2
                     WHERE id = ?1 AND conversation_id = ?3 AND status = 'proposed'",
                    params![plan_revision_id, resolved_at, conversation_id],
                )?
            } else {
                transaction.execute(
                    "UPDATE plan_revisions SET status = 'rejected', approved_at = NULL
                     WHERE id = ?1 AND conversation_id = ?2 AND status = 'proposed'",
                    params![plan_revision_id, conversation_id],
                )?
            };
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            let record = transaction.query_row(
                "SELECT id, goal_id, conversation_id, revision, title, summary, tasks_json, status,
                     created_by, created_at, approved_at FROM plan_revisions WHERE id = ?1",
                [plan_revision_id],
                plan_revision_from_row,
            )?;
            transaction.commit()?;
            Ok(record)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_review_finding(
        &self,
        conversation_id: &str,
        goal_id: &str,
        task_id: Option<&str>,
        plan_revision_id: Option<&str>,
        severity: &str,
        category: &str,
        title: &str,
        detail: &str,
        status: &str,
        created_by: &str,
    ) -> Result<ReviewFindingRecord, String> {
        let id = Uuid::new_v4().to_string();
        let created_at = now_ms().to_string();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO review_findings(id, goal_id, task_id, plan_revision_id, conversation_id,
                     severity, category, title, detail, status, created_by, created_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     CASE WHEN ?10 IN ('resolved', 'waived') THEN ?12 ELSE NULL END)",
                params![id, goal_id, task_id, plan_revision_id, conversation_id, severity, category, title, detail, status, created_by, created_at],
            )?;
            connection.query_row("SELECT id, goal_id, task_id, plan_revision_id, conversation_id, severity, category, title, detail, status, created_by, created_at, resolved_at FROM review_findings WHERE id = ?1", [&id], review_finding_from_row)
        })
    }

    pub fn resolve_review_finding(
        &self,
        id: &str,
        status: &str,
    ) -> Result<Option<ReviewFindingRecord>, String> {
        let resolved_at = now_ms().to_string();
        self.with_connection(|connection| {
            connection.execute("UPDATE review_findings SET status = ?2, resolved_at = ?3 WHERE id = ?1", params![id, status, resolved_at])?;
            connection.query_row("SELECT id, goal_id, task_id, plan_revision_id, conversation_id, severity, category, title, detail, status, created_by, created_at, resolved_at FROM review_findings WHERE id = ?1", [id], review_finding_from_row).optional()
        })
    }

    pub fn create_acceptance(
        &self,
        conversation_id: &str,
        goal_id: &str,
        plan_revision_id: Option<&str>,
        status: &str,
        summary: &str,
        checks: Value,
        reviewer: &str,
    ) -> Result<AcceptanceRecord, String> {
        let id = Uuid::new_v4().to_string();
        let created_at = now_ms().to_string();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO acceptances(id, goal_id, plan_revision_id, conversation_id, status,
                     summary, checks_json, reviewer, created_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     CASE WHEN ?5 IN ('accepted', 'rejected') THEN ?9 ELSE NULL END)",
                params![id, goal_id, plan_revision_id, conversation_id, status, summary, checks.to_string(), reviewer, created_at],
            )?;
            connection.query_row("SELECT id, goal_id, plan_revision_id, conversation_id, status, summary, checks_json, reviewer, created_at, resolved_at FROM acceptances WHERE id = ?1", [&id], acceptance_from_row)
        })
    }

    pub fn load_a1_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<
        (
            Vec<PlanRevisionRecord>,
            Vec<ReviewFindingRecord>,
            Vec<AcceptanceRecord>,
        ),
        String,
    > {
        self.with_connection(|connection| {
            let plans = connection.prepare("SELECT id, goal_id, conversation_id, revision, title, summary, tasks_json, status, created_by, created_at, approved_at FROM plan_revisions WHERE conversation_id = ?1 ORDER BY revision DESC")?
                .query_map([conversation_id], plan_revision_from_row)?.collect::<Result<Vec<_>, _>>()?;
            let findings = connection.prepare("SELECT id, goal_id, task_id, plan_revision_id, conversation_id, severity, category, title, detail, status, created_by, created_at, resolved_at FROM review_findings WHERE conversation_id = ?1 ORDER BY created_at DESC")?
                .query_map([conversation_id], review_finding_from_row)?.collect::<Result<Vec<_>, _>>()?;
            let acceptances = connection.prepare("SELECT id, goal_id, plan_revision_id, conversation_id, status, summary, checks_json, reviewer, created_at, resolved_at FROM acceptances WHERE conversation_id = ?1 ORDER BY created_at DESC")?
                .query_map([conversation_id], acceptance_from_row)?.collect::<Result<Vec<_>, _>>()?;
            Ok((plans, findings, acceptances))
        })
    }
}

fn plan_revision_from_row(row: &Row<'_>) -> rusqlite::Result<PlanRevisionRecord> {
    Ok(PlanRevisionRecord {
        id: row.get(0)?,
        goal_id: row.get(1)?,
        conversation_id: row.get(2)?,
        revision: row.get(3)?,
        title: row.get(4)?,
        summary: row.get(5)?,
        tasks: json_value(row.get(6)?),
        status: row.get(7)?,
        created_by: row.get(8)?,
        created_at: row.get(9)?,
        approved_at: row.get(10)?,
    })
}

fn review_finding_from_row(row: &Row<'_>) -> rusqlite::Result<ReviewFindingRecord> {
    Ok(ReviewFindingRecord {
        id: row.get(0)?,
        goal_id: row.get(1)?,
        task_id: row.get(2)?,
        plan_revision_id: row.get(3)?,
        conversation_id: row.get(4)?,
        severity: row.get(5)?,
        category: row.get(6)?,
        title: row.get(7)?,
        detail: row.get(8)?,
        status: row.get(9)?,
        created_by: row.get(10)?,
        created_at: row.get(11)?,
        resolved_at: row.get(12)?,
    })
}

fn acceptance_from_row(row: &Row<'_>) -> rusqlite::Result<AcceptanceRecord> {
    Ok(AcceptanceRecord {
        id: row.get(0)?,
        goal_id: row.get(1)?,
        plan_revision_id: row.get(2)?,
        conversation_id: row.get(3)?,
        status: row.get(4)?,
        summary: row.get(5)?,
        checks: json_value(row.get(6)?),
        reviewer: row.get(7)?,
        created_at: row.get(8)?,
        resolved_at: row.get(9)?,
    })
}

fn json_value(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{CreateGoalInput, GoalStatus};
    use serde_json::json;

    #[test]
    fn plan_revisions_require_host_resolution_and_preserve_approved_history() {
        let path = std::env::temp_dir().join(format!("fox-a1-plan-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("A1 plan"), None, None)
            .expect("create conversation");
        let run = database
            .create_run(&conversation.id, "create plan", None)
            .expect("create run")
            .run;
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "A1 plan".to_owned(),
                objective: "verify plan approval history".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run.id.clone(),
            })
            .expect("create goal");

        let first = database
            .create_plan_revision(
                &conversation.id,
                &goal.id,
                "v1",
                "initial",
                json!([]),
                &run.id,
            )
            .expect("create first plan");
        assert_eq!(first.status, "proposed");
        let first = database
            .resolve_plan_revision(&conversation.id, &first.id, "approved")
            .expect("approve first plan");
        assert_eq!(first.status, "approved");

        let second = database
            .create_plan_revision(
                &conversation.id,
                &goal.id,
                "v2",
                "candidate",
                json!([]),
                &run.id,
            )
            .expect("create second plan");
        let (plans, _, _) = database
            .load_a1_snapshot(&conversation.id)
            .expect("load plans");
        assert_eq!(
            plans
                .iter()
                .find(|plan| plan.id == first.id)
                .unwrap()
                .status,
            "approved"
        );
        database
            .resolve_plan_revision(&conversation.id, &second.id, "rejected")
            .expect("reject second plan");

        let third = database
            .create_plan_revision(
                &conversation.id,
                &goal.id,
                "v3",
                "replacement",
                json!([]),
                &run.id,
            )
            .expect("create third plan");
        database
            .resolve_plan_revision(&conversation.id, &third.id, "approved")
            .expect("approve third plan");
        let (plans, _, _) = database
            .load_a1_snapshot(&conversation.id)
            .expect("load history");
        assert_eq!(
            plans
                .iter()
                .find(|plan| plan.id == first.id)
                .unwrap()
                .status,
            "superseded"
        );
        assert_eq!(
            plans
                .iter()
                .find(|plan| plan.id == second.id)
                .unwrap()
                .status,
            "rejected"
        );
        assert_eq!(
            plans
                .iter()
                .find(|plan| plan.id == third.id)
                .unwrap()
                .status,
            "approved"
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
