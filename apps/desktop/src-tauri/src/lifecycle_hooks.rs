use crate::database::Database;
use serde_json::{json, Value};

#[derive(Debug, Default)]
pub struct BeforeToolDecision {
    pub blocked: Option<String>,
    pub requires_approval: bool,
    pub approval_reason: Option<String>,
    pub annotations: Vec<String>,
}

pub fn before_tool(
    database: &Database,
    run_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    input: &Value,
) -> Result<BeforeToolDecision, String> {
    let mut decision = BeforeToolDecision::default();
    let input_keys = input
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for hook in database
        .list_lifecycle_hooks()?
        .into_iter()
        .filter(|hook| hook.enabled && hook.event == "before_tool")
        .filter(|hook| matcher_matches(&hook.matcher, tool_name))
    {
        database.record_lifecycle_hook_execution(
            &hook.id,
            Some(run_id),
            Some(tool_call_id),
            "before_tool",
            Some(tool_name),
            &hook.action,
            "applied",
            &json!({ "inputKeys": input_keys }),
        )?;
        let reason = if hook.reason.trim().is_empty() {
            hook.name.clone()
        } else {
            hook.reason.clone()
        };
        match hook.action.as_str() {
            "block" => {
                decision.blocked.get_or_insert(reason);
            }
            "require_approval" => {
                decision.requires_approval = true;
                decision.approval_reason.get_or_insert(reason);
            }
            "annotate" => decision.annotations.push(reason),
            _ => {}
        }
    }
    Ok(decision)
}

pub fn after_tool(
    database: &Database,
    run_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    succeeded: bool,
) -> Result<Vec<String>, String> {
    let mut annotations = Vec::new();
    for hook in database
        .list_lifecycle_hooks()?
        .into_iter()
        .filter(|hook| hook.enabled && hook.event == "after_tool")
        .filter(|hook| matcher_matches(&hook.matcher, tool_name))
    {
        let reason = if hook.reason.trim().is_empty() {
            hook.name.clone()
        } else {
            hook.reason.clone()
        };
        database.record_lifecycle_hook_execution(
            &hook.id,
            Some(run_id),
            Some(tool_call_id),
            "after_tool",
            Some(tool_name),
            &hook.action,
            "applied",
            &json!({ "succeeded": succeeded }),
        )?;
        if hook.action == "annotate" {
            annotations.push(reason);
        }
    }
    Ok(annotations)
}

pub fn run_event(
    database: &Database,
    event: &str,
    run_id: &str,
    matcher_value: &str,
    details: &Value,
) -> Result<Vec<String>, String> {
    let mut annotations = Vec::new();
    for hook in database
        .list_lifecycle_hooks()?
        .into_iter()
        .filter(|hook| hook.enabled && hook.event == event)
        .filter(|hook| matcher_matches(&hook.matcher, matcher_value))
    {
        database.record_lifecycle_hook_execution(
            &hook.id,
            Some(run_id),
            None,
            event,
            None,
            &hook.action,
            "applied",
            details,
        )?;
        if hook.action == "annotate" {
            annotations.push(if hook.reason.trim().is_empty() {
                hook.name
            } else {
                hook.reason
            });
        }
    }
    Ok(annotations)
}

fn matcher_matches(patterns: &str, value: &str) -> bool {
    patterns
        .split([',', '|'])
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| glob_matches(pattern, value))
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let starts = pattern.starts_with('*');
    let ends = pattern.ends_with('*');
    let needle = pattern.trim_matches('*');
    match (starts, ends) {
        (true, true) => value.contains(needle),
        (true, false) => value.ends_with(needle),
        (false, true) => value.starts_with(needle),
        (false, false) => value == needle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn declarative_matchers_support_exact_prefix_suffix_and_lists() {
        assert!(matcher_matches("run_command, call_*", "call_mcp_tool"));
        assert!(matcher_matches("*_write", "git_write"));
        assert!(matcher_matches("*mcp*", "call_mcp_tool"));
        assert!(!matcher_matches("read,write", "run_command"));
    }

    #[test]
    fn applies_before_and_after_tool_actions_and_records_audit_rows() {
        let path = std::env::temp_dir().join(format!("fox-hook-test-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        let conversation = database
            .create_conversation("fox-general", Some("Hook test"), None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "test hooks", None)
            .expect("create run");
        database
            .save_lifecycle_hook(
                "approval-hook",
                "Approval",
                "before_tool",
                "run_*",
                "require_approval",
                "review command",
                true,
                10,
            )
            .unwrap();
        database
            .save_lifecycle_hook(
                "block-hook",
                "Block",
                "before_tool",
                "run_command",
                "block",
                "blocked by policy",
                true,
                20,
            )
            .unwrap();
        database
            .save_lifecycle_hook(
                "after-hook",
                "After",
                "after_tool",
                "run_command",
                "annotate",
                "verify output",
                true,
                30,
            )
            .unwrap();
        let decision = before_tool(
            &database,
            &started.run.id,
            "tool-call-1",
            "run_command",
            &json!({ "command": "cargo test" }),
        )
        .expect("evaluate before tool");
        assert!(decision.requires_approval);
        assert_eq!(decision.approval_reason.as_deref(), Some("review command"));
        assert_eq!(decision.blocked.as_deref(), Some("blocked by policy"));
        assert_eq!(
            after_tool(
                &database,
                &started.run.id,
                "tool-call-1",
                "run_command",
                true,
            )
            .unwrap(),
            vec!["verify output"]
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
