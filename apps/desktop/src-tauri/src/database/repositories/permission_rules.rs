//! A1: project permission rules, the four-mode conversation access, the
//! per-project full-access confirmation and the auto-allow audit log.
use super::{now_ms, Database};
use crate::runtime_host::approval_policy::{AccessMode, PolicyDecision, PolicyRule, RuleEffect};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRuleRecord {
    pub id: String,
    pub project_id: String,
    pub effect: String,
    pub tool: String,
    pub pattern: String,
    pub source: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

impl PermissionRuleRecord {
    pub fn policy_rule(&self) -> Option<PolicyRule> {
        Some(PolicyRule {
            id: self.id.clone(),
            effect: RuleEffect::parse(&self.effect).ok()?,
            tool: self.tool.clone(),
            pattern: self.pattern.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAuditRecord {
    pub id: i64,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool: String,
    pub mode: String,
    pub class: String,
    pub risk: String,
    pub reason: String,
    pub rule_ids: Vec<String>,
    pub findings: Vec<String>,
    pub summary: String,
    pub created_at: i64,
}

/// The access a conversation's next tool call is decided under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationAccess {
    pub project_id: Option<String>,
    pub project_root: Option<String>,
    pub stored_mode: String,
    pub full_access: bool,
    pub mode: AccessMode,
    pub project_full_access_confirmed: bool,
}

const RULE_COLUMNS: &str = "id, project_id, effect, tool, pattern, source, created_at, revoked_at";

fn map_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<PermissionRuleRecord> {
    Ok(PermissionRuleRecord {
        id: row.get(0)?,
        project_id: row.get(1)?,
        effect: row.get(2)?,
        tool: row.get(3)?,
        pattern: row.get(4)?,
        source: row.get(5)?,
        created_at: row.get(6)?,
        revoked_at: row.get(7)?,
    })
}

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

impl Database {
    /// Effective access, read fresh for every tool call (a mode switched in the
    /// middle of a Run applies from the next tool call on).
    pub fn conversation_access(&self, conversation_id: &str) -> Result<ConversationAccess, String> {
        let row = self.with_connection(|connection| {
            connection.query_row(
                "SELECT c.project_id, COALESCE(p.root_path, c.project_root),
                        COALESCE(kep.mode, c.permission_mode, 'ask'), c.full_access,
                        p.full_access_confirmed_at IS NOT NULL
                 FROM conversations c
                 LEFT JOIN projects p ON p.id = c.project_id
                 LEFT JOIN kernel_execution_policies kep ON kep.conversation_id = c.id
                 WHERE c.id = ?1",
                [conversation_id],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?,
                          row.get::<_, String>(2)?, row.get::<_, bool>(3)?, row.get::<_, bool>(4)?)),
            )
        })?;
        let (project_id, project_root, stored_mode, full_access, confirmed) = row;
        let mode = AccessMode::from_stored(&stored_mode, full_access)?;
        Ok(ConversationAccess { project_id, project_root, stored_mode, full_access, mode, project_full_access_confirmed: confirmed })
    }

    pub fn project_full_access_confirmed(&self, project_id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT full_access_confirmed_at IS NOT NULL FROM projects WHERE id = ?1",
                [project_id],
                |row| row.get(0),
            )
        })
    }

    /// Remember that the user confirmed full access for this project once.
    pub fn confirm_project_full_access(&self, project_id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE projects SET full_access_confirmed_at = COALESCE(full_access_confirmed_at, ?2) WHERE id = ?1",
                params![project_id, now_ms()],
            )?;
            if changed != 1 {
                return Err(invalid("project.not_found"));
            }
            Ok(())
        })
    }

    pub fn permission_rules_for_project(&self, project_id: &str) -> Result<Vec<PermissionRuleRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {RULE_COLUMNS} FROM permission_rules
                 WHERE project_id = ?1 AND revoked_at IS NULL
                 ORDER BY created_at ASC, id ASC"
            ))?;
            let rules = statement.query_map([project_id], map_rule)?.collect();
            rules
        })
    }

    /// The live rules the engine sees for a conversation (none without a project).
    pub fn policy_rules_for_conversation(&self, conversation_id: &str) -> Result<Vec<PolicyRule>, String> {
        let project_id: Option<String> = self.with_connection(|connection| {
            connection
                .query_row("SELECT project_id FROM conversations WHERE id = ?1", [conversation_id], |row| row.get(0))
                .optional()
                .map(Option::flatten)
        })?;
        let Some(project_id) = project_id else { return Ok(Vec::new()) };
        Ok(self.permission_rules_for_project(&project_id)?.iter().filter_map(PermissionRuleRecord::policy_rule).collect())
    }

    /// Add (or return the existing live) rule. The pattern must be one plain
    /// command prefix.
    pub fn add_permission_rule(&self, project_id: &str, effect: &str, tool: &str, pattern: &str, source: &str) -> Result<PermissionRuleRecord, String> {
        RuleEffect::parse(effect)?;
        if !matches!(source, "user" | "builtin") {
            return Err("permission.invalid_rule: unknown rule source".into());
        }
        if tool != "*" && !crate::runtime_host::approval_policy::is_command_tool(tool) {
            return Err(format!("permission.invalid_rule: rules apply to command tools, not {tool}"));
        }
        let words = crate::runtime_host::approval_policy::parse_rule_pattern(pattern)?;
        let pattern = words.join(" ");
        let id = uuid::Uuid::new_v4().to_string();
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let exists: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)", [project_id], |row| row.get(0))?;
            if !exists {
                return Err(invalid("project.not_found"));
            }
            transaction.execute(
                "INSERT INTO permission_rules(id, project_id, effect, tool, pattern, source, created_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(project_id, effect, tool, pattern) WHERE revoked_at IS NULL DO NOTHING",
                params![id, project_id, effect, tool, pattern, source, now_ms()],
            )?;
            let record = transaction.query_row(
                &format!("SELECT {RULE_COLUMNS} FROM permission_rules
                          WHERE project_id = ?1 AND effect = ?2 AND tool = ?3 AND pattern = ?4 AND revoked_at IS NULL"),
                params![project_id, effect, tool, pattern],
                map_rule,
            )?;
            transaction.commit()?;
            Ok(record)
        })
    }

    pub fn revoke_permission_rule(&self, rule_id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE permission_rules SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
                params![rule_id, now_ms()],
            )? == 1)
        })
    }

    /// One row per automatic allow of a non-read tool call.
    pub fn record_permission_audit(&self, conversation_id: &str, run_id: Option<&str>, tool_call_id: Option<&str>,
        tool: &str, decision: &PolicyDecision, summary: &str) -> Result<(), String> {
        let reason = decision.allow_reason.as_ref().map(|reason| reason.as_str()).unwrap_or("unknown");
        let summary: String = summary.chars().take(500).collect();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO permission_audit(conversation_id, run_id, tool_call_id, tool, mode, class, risk,
                                              reason, rule_ids_json, findings_json, summary, created_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![conversation_id, run_id, tool_call_id, tool, decision.mode.as_str(), decision.class.as_str(),
                        decision.risk.as_str(), reason, json!(decision.rule_ids()).to_string(),
                        json!(decision.findings).to_string(), summary, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn approval_record(&self, approval_id: &str) -> Result<crate::database::ApprovalRecord, String> {
        self.with_connection(|connection| super::query_approval(connection, approval_id))
    }

    pub fn permission_audit_list(&self, run_id: Option<&str>, conversation_id: Option<&str>, limit: usize) -> Result<Vec<PermissionAuditRecord>, String> {
        if run_id.is_none() && conversation_id.is_none() {
            return Err("permission.audit_scope_required".into());
        }
        let limit = limit.clamp(1, 1000) as i64;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, conversation_id, run_id, tool_call_id, tool, mode, class, risk, reason,
                        rule_ids_json, findings_json, summary, created_at
                 FROM permission_audit
                 WHERE (?1 IS NULL OR run_id = ?1) AND (?2 IS NULL OR conversation_id = ?2)
                 ORDER BY id ASC LIMIT ?3",
            )?;
            let rows = statement.query_map(params![run_id, conversation_id, limit], |row| {
                let rule_ids: String = row.get(9)?;
                let findings: String = row.get(10)?;
                Ok(PermissionAuditRecord {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    run_id: row.get(2)?,
                    tool_call_id: row.get(3)?,
                    tool: row.get(4)?,
                    mode: row.get(5)?,
                    class: row.get(6)?,
                    risk: row.get(7)?,
                    reason: row.get(8)?,
                    rule_ids: serde_json::from_str(&rule_ids).unwrap_or_default(),
                    findings: serde_json::from_str(&findings).unwrap_or_default(),
                    summary: row.get(11)?,
                    created_at: row.get(12)?,
                })
            })?;
            rows.collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_host::approval_policy::{decide, PolicyRequest, ToolSubject};

    struct TempRoot(std::path::PathBuf);
    impl TempRoot {
        fn path(&self) -> &std::path::Path { &self.0 }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn database_with_project() -> (Database, String, String, TempRoot) {
        let database = Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        let root = TempRoot(std::env::temp_dir().join(format!("fox-a1-rules-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(root.path()).expect("root");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("A1"), Some(root.path().to_str().unwrap()), Some("ask"))
            .expect("conversation");
        let project = conversation.project_id.clone().expect("project");
        (database, project, conversation.id, root)
    }

    #[test]
    fn a1_permission_rules_add_dedupe_revoke_and_feed_the_engine() {
        let (database, project, conversation, root) = database_with_project();
        let first = database.add_permission_rule(&project, "allow", "run_command", "  npm   test ", "user").unwrap();
        assert_eq!(first.pattern, "npm test");
        let again = database.add_permission_rule(&project, "allow", "run_command", "npm test", "user").unwrap();
        assert_eq!(first.id, again.id, "a live duplicate is returned, not added twice");
        assert!(database.add_permission_rule(&project, "allow", "write_file", "x", "user").is_err());
        assert!(database.add_permission_rule(&project, "allow", "run_command", "npm test && rm x", "user").is_err());
        let rules = database.policy_rules_for_conversation(&conversation).unwrap();
        assert_eq!(rules.len(), 1);
        let subject = ToolSubject::Command { command: "npm test -- a".into(), cwd: root.path().to_path_buf(), model_risk: None };
        let decision = decide(&PolicyRequest { tool: "run_command", subject: &subject, mode: AccessMode::Ask,
            project_root: Some(root.path()), home: None, rules: &rules });
        assert!(decision.is_allow());
        database.record_permission_audit(&conversation, None, Some("call-1"), "run_command", &decision, "npm test -- a").unwrap();
        let audit = database.permission_audit_list(None, Some(&conversation), 10).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].reason, "rule");
        assert_eq!(audit[0].rule_ids, vec![first.id.clone()]);
        assert!(database.revoke_permission_rule(&first.id).unwrap());
        assert!(!database.revoke_permission_rule(&first.id).unwrap());
        assert!(database.policy_rules_for_conversation(&conversation).unwrap().is_empty());
    }

    #[test]
    fn a1_permission_rules_full_access_is_a_flag_next_to_allow_and_any_other_change_clears_it() {
        let (database, project, conversation, _root) = database_with_project();
        let access = database.conversation_access(&conversation).unwrap();
        assert_eq!((access.mode, access.full_access, access.project_full_access_confirmed), (AccessMode::Ask, false, false));
        let policy = database.execution_policy(&conversation).unwrap();
        assert!(database.change_execution_policy_with_access(&conversation, "r1", policy.version, "ask", true).is_err());
        let policy = database.change_execution_policy_with_access(&conversation, "r1", policy.version, "allow", true).unwrap();
        assert_eq!(database.conversation_access(&conversation).unwrap().mode, AccessMode::FullAccess);
        let policy = database.change_execution_policy(&conversation, "r2", policy.version, "allow").unwrap();
        let access = database.conversation_access(&conversation).unwrap();
        assert_eq!((access.mode, access.full_access), (AccessMode::Auto, false));
        let _ = policy;
        database.confirm_project_full_access(&project).unwrap();
        assert!(database.project_full_access_confirmed(&project).unwrap());
        assert!(database.conversation_access(&conversation).unwrap().project_full_access_confirmed);
    }
}
