//! Plan C2: the external pi is answered by the Host's own approval engine.
//!
//! pi has no permission system of its own; Fox's approval gate extension
//! raises every tool call as a `session/request_permission`. The Host answers
//! it with the SAME engine as a PiNative conversation
//! ([`crate::runtime_host::approval_policy`]): block rules (built-in dangerous
//! deletions and the project's `block` rules), protected / outside paths, the
//! tier, then the project's `allow` / `ask` rules. The tier is the
//! conversation's choice in pi's list:
//!
//! | pi option | engine mode |
//! |---|---|
//! | `read_only` | 只读 (reads allowed, the rest refused) |
//! | `ask` | 询问 |
//! | `allow` | 自动 |
//! | `full_access` | 完全访问 (block rules still apply) |
//!
//! pi's tools are named for the engine the way PiNative's are: `read` / `grep`
//! / `find` / `ls` are reads, `write` / `edit` are writes of their `path`,
//! `bash` is `run_command` (so a project rule written for `run_command`
//! covers it). A request that is not a Fox gate approval (another extension's
//! dialog) is judged as an unknown tool. Only the card decides what the
//! engine leaves to the user; an engine failure is a fail-closed reject.
use crate::runtime_host::approval_policy::{self, AccessMode, PolicyDecision, PolicyRule, ToolClass, ToolSubject, Verdict};
use fox_agent_backend::backends::pi::{parse_tool_approval, ToolApproval};
use fox_agent_backend::external::HostPermissionAnswer;
use fox_agent_backend::native_permission::{PI_ALLOW, PI_FULL, PI_READ_ONLY};
use fox_agent_backend::projection::{allow_option, reject_option, AutoPermission, PermissionOption};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The engine mode of a pi option id (unknown = 询问, never wider).
pub fn pi_access_mode(native_mode: &str) -> AccessMode {
    match native_mode.trim() {
        PI_READ_ONLY => AccessMode::ReadOnly,
        PI_ALLOW => AccessMode::Auto,
        PI_FULL => AccessMode::FullAccess,
        _ => AccessMode::Ask,
    }
}

fn resolve(base: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    }
}

/// The engine's tool name and subject of one pi gate request.
pub fn pi_subject(approval: Option<&ToolApproval>, project_root: Option<&Path>) -> (String, ToolSubject) {
    let Some(approval) = approval else {
        return ("agent_permission".to_owned(), ToolSubject::Other);
    };
    let base = approval
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| project_root.map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let text = |key: &str| approval.input.get(key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty());
    match approval.tool_name.as_str() {
        "read" | "grep" | "find" | "ls" => (approval.tool_name.clone(), ToolSubject::Read),
        "write" | "edit" => {
            let tool = if approval.tool_name == "write" { "write_file" } else { "edit_file" };
            match text("path").or_else(|| text("file_path")) {
                Some(path) => (tool.to_owned(), ToolSubject::Write { target: resolve(&base, path) }),
                None => (tool.to_owned(), ToolSubject::Other),
            }
        }
        "bash" => match text("command") {
            Some(command) => (
                "run_command".to_owned(),
                ToolSubject::Command { command: command.to_owned(), cwd: base, model_risk: approval.input.get("risk").and_then(Value::as_str).map(str::to_owned) },
            ),
            None => ("run_command".to_owned(), ToolSubject::Other),
        },
        other => (other.to_owned(), ToolSubject::Other),
    }
}

/// One engine decision for a pi request (no I/O).
pub fn decide_pi(native_mode: &str, request: &Value, project_root: Option<&Path>, home: Option<&Path>, rules: &[PolicyRule]) -> (String, PolicyDecision) {
    let approval = parse_tool_approval(request);
    let (tool, subject) = pi_subject(approval.as_ref(), project_root);
    let decision = approval_policy::decide(&approval_policy::PolicyRequest {
        tool: &tool,
        subject: &subject,
        mode: pi_access_mode(native_mode),
        project_root,
        home,
        rules,
    });
    (tool, decision)
}

/// The native answer of an engine decision, with its summary for the run
/// event and the audit (never tool input).
pub fn answer_from(tool: &str, decision: &PolicyDecision, options: &[PermissionOption]) -> HostPermissionAnswer {
    let reject = || AutoPermission::Reject { option_id: reject_option(options).map(|option| option.option_id.clone()) };
    let (verdict, auto) = match &decision.verdict {
        // An allow without a one-shot allow option cannot be written: refuse.
        Verdict::Allow => ("allow", allow_option(options).map(|option| AutoPermission::Allow { option_id: option.option_id.clone() }).unwrap_or_else(reject)),
        Verdict::Ask => ("ask", AutoPermission::Ask),
        Verdict::Deny { .. } => ("deny", reject()),
    };
    let mut policy = decision.card_json();
    policy["tool"] = json!(tool);
    policy["verdict"] = json!(verdict);
    policy["allowReason"] = json!(decision.allow_reason.as_ref().map(|reason| reason.as_str()));
    policy["ruleIds"] = json!(decision.rule_ids());
    if let Verdict::Deny { code, message } = &decision.verdict {
        policy["denyCode"] = json!(code);
        policy["denyMessage"] = json!(message);
    }
    HostPermissionAnswer { decision: auto, policy }
}

/// The fail-closed answer when the engine could not be consulted.
pub fn fail_closed(error: &str, options: &[PermissionOption]) -> HostPermissionAnswer {
    HostPermissionAnswer {
        decision: AutoPermission::Reject { option_id: reject_option(options).map(|option| option.option_id.clone()) },
        policy: json!({ "verdict": "deny", "failClosed": true, "error": error }),
    }
}

/// Answer one pi request for `conversation_id` with its project root and
/// rules as they are now; every automatic allow of a non-read call leaves one
/// row in the same audit as PiNative.
pub fn answer_pi_request(
    database: &crate::database::Database,
    run_id: &str,
    conversation_id: &str,
    native_mode: &str,
    request: &Value,
    options: &[PermissionOption],
) -> Result<HostPermissionAnswer, String> {
    let context = database.external_conversation_context(conversation_id)?.ok_or_else(|| "conversation not found".to_owned())?;
    let rules = database.policy_rules_for_conversation(conversation_id)?;
    // The ACP cwd is the canonical project root; compare with it the same way.
    let root = context.project_root.as_deref().map(|root| {
        let root = PathBuf::from(root);
        root.canonicalize().unwrap_or(root)
    });
    let home = approval_policy::host_home();
    let (tool, decision) = decide_pi(native_mode, request, root.as_deref(), home.as_deref(), &rules);
    if decision.is_allow() && decision.class != ToolClass::Read {
        let approval = parse_tool_approval(request);
        let summary = approval
            .as_ref()
            .and_then(|approval| approval.input.get("command").or_else(|| approval.input.get("path")).and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| tool.clone());
        let tool_call_id = approval.as_ref().map(|approval| approval.tool_call_id.clone()).filter(|id| !id.is_empty());
        database.record_permission_audit(conversation_id, Some(run_id), tool_call_id.as_deref(), &tool, &decision, &summary)?;
    }
    Ok(answer_from(&tool, &decision, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_host::approval_policy::RuleEffect;

    fn root() -> PathBuf {
        if cfg!(windows) { PathBuf::from("C:/work/proj") } else { PathBuf::from("/work/proj") }
    }
    fn home() -> PathBuf {
        if cfg!(windows) { PathBuf::from("C:/Users/fox") } else { PathBuf::from("/home/fox") }
    }
    fn options() -> Vec<PermissionOption> {
        vec![
            PermissionOption { option_id: "yes".into(), name: "Allow".into(), kind: "allow_once".into() },
            PermissionOption { option_id: "no".into(), name: "Reject".into(), kind: "reject_once".into() },
        ]
    }
    /// A Fox gate request exactly as pi-acp 0.0.34 sends it.
    fn gate(tool: &str, input: Value) -> Value {
        let message = json!({ "fox": "tool-approval", "v": 1, "toolCallId": "call_1", "toolName": tool, "input": input, "cwd": root() }).to_string();
        json!({
            "sessionId": "s1",
            "toolCall": { "toolCallId": "pi-ui-1", "title": format!("Fox approval: {tool}"), "kind": "other", "status": "pending",
                "rawInput": { "method": "confirm", "title": format!("Fox approval: {tool}"), "message": message } },
            "options": [{ "optionId": "yes", "name": "Yes", "kind": "allow_once" }, { "optionId": "no", "name": "No", "kind": "reject_once" }]
        })
    }
    fn verdict(mode: &str, request: &Value, rules: &[PolicyRule]) -> &'static str {
        let (tool, decision) = decide_pi(mode, request, Some(&root()), Some(&home()), rules);
        match answer_from(&tool, &decision, &options()).decision {
            AutoPermission::Allow { .. } => "allow",
            AutoPermission::Ask => "ask",
            AutoPermission::Reject { .. } => "deny",
        }
    }
    fn rule(effect: RuleEffect, tool: &str, pattern: &str) -> PolicyRule {
        PolicyRule { id: format!("r-{pattern}"), effect, tool: tool.into(), pattern: pattern.into() }
    }

    #[test]
    fn pi_options_map_to_the_engine_modes() {
        assert_eq!(pi_access_mode("read_only"), AccessMode::ReadOnly);
        assert_eq!(pi_access_mode("ask"), AccessMode::Ask);
        assert_eq!(pi_access_mode("allow"), AccessMode::Auto);
        assert_eq!(pi_access_mode("full_access"), AccessMode::FullAccess);
        assert_eq!(pi_access_mode("junk"), AccessMode::Ask, "unknown never widens");
    }

    /// The tier table of plan C2 for pi, the same rows as PiNative.
    #[test]
    fn pi_tool_calls_follow_the_pi_native_decision_table() {
        let read = gate("read", json!({ "path": "src/main.rs" }));
        let write = gate("write", json!({ "path": "src/new.rs", "content": "x" }));
        let edit_outside = gate("edit", json!({ "path": "../other/x.rs" }));
        let low = gate("bash", json!({ "command": "ls -la" }));
        let high = gate("bash", json!({ "command": "git push origin main" }));
        let rows: [(&Value, [&str; 4]); 5] = [
            //                 只读     询问   自动     完全访问
            (&read, ["allow", "allow", "allow", "allow"]),
            (&write, ["deny", "ask", "allow", "allow"]),
            (&edit_outside, ["deny", "ask", "ask", "allow"]),
            (&low, ["deny", "ask", "allow", "allow"]),
            (&high, ["deny", "ask", "ask", "allow"]),
        ];
        for (request, expected) in rows {
            for (mode, want) in ["read_only", "ask", "allow", "full_access"].into_iter().zip(expected) {
                assert_eq!(verdict(mode, request, &[]), want, "{mode}: {request}");
            }
        }
    }

    /// Block rules (built-in and project) refuse in every tier, full access
    /// included; allow / ask rules written for `run_command` cover pi's bash.
    #[test]
    fn project_rules_and_block_rules_apply_to_pi_like_pi_native() {
        let delete_root = gate("bash", json!({ "command": format!("rm -rf {}", root().display()) }));
        for mode in ["read_only", "ask", "allow", "full_access"] {
            assert_eq!(verdict(mode, &delete_root, &[]), "deny", "{mode}: built-in block");
        }
        let block = [rule(RuleEffect::Block, "run_command", "curl")];
        let curl = gate("bash", json!({ "command": "curl https://example.com" }));
        assert_eq!(verdict("full_access", &curl, &block), "deny", "project block rule in full access");
        let allow = [rule(RuleEffect::Allow, "run_command", "npm test")];
        assert_eq!(verdict("ask", &gate("bash", json!({ "command": "npm test -- foo" })), &allow), "allow");
        assert_eq!(verdict("ask", &gate("bash", json!({ "command": "npm testx" })), &allow), "ask", "token prefix only");
        assert_eq!(verdict("ask", &gate("bash", json!({ "command": "npm test && git push" })), &allow), "ask", "every segment must be covered");
        let ask = [rule(RuleEffect::Ask, "run_command", "ls")];
        assert_eq!(verdict("allow", &gate("bash", json!({ "command": "ls" })), &ask), "ask", "an ask rule forces the card in 自动");
        // The model may raise the risk, never lower it.
        assert_eq!(verdict("allow", &gate("bash", json!({ "command": "ls", "risk": "high" })), &[]), "ask");
    }

    #[test]
    fn requests_that_are_not_gate_approvals_are_unknown_tools() {
        let other = json!({ "sessionId": "s1", "toolCall": { "toolCallId": "x", "title": "Delete?", "rawInput": { "method": "confirm", "message": "really?" } },
            "options": [{ "optionId": "yes", "name": "Yes", "kind": "allow_once" }, { "optionId": "no", "name": "No", "kind": "reject_once" }] });
        assert_eq!(verdict("ask", &other, &[]), "ask");
        assert_eq!(verdict("allow", &other, &[]), "ask");
        assert_eq!(verdict("full_access", &other, &[]), "allow");
        let unknown_tool = gate("web_fetch_x", json!({}));
        assert_eq!(verdict("allow", &unknown_tool, &[]), "ask");
    }

    #[test]
    fn the_answer_carries_a_summary_and_never_the_input() {
        let request = gate("bash", json!({ "command": "git push origin main", "secret": "sk-not-real" }));
        let (tool, decision) = decide_pi("allow", &request, Some(&root()), Some(&home()), &[]);
        let answer = answer_from(&tool, &decision, &options());
        assert_eq!(answer.decision, AutoPermission::Ask);
        assert_eq!(answer.policy["tool"], "run_command");
        assert_eq!(answer.policy["mode"], "auto");
        assert_eq!(answer.policy["risk"], "high");
        assert!(!answer.policy.to_string().contains("sk-not-real"));
        let (tool, decision) = decide_pi("read_only", &gate("write", json!({ "path": "a.txt" })), Some(&root()), Some(&home()), &[]);
        let answer = answer_from(&tool, &decision, &options());
        assert_eq!(answer.decision, AutoPermission::Reject { option_id: Some("no".into()) });
        assert_eq!(answer.policy["denyCode"], "permission.read_only");
        // An allow that cannot be written as a one-shot allow is refused.
        let (tool, decision) = decide_pi("full_access", &gate("write", json!({ "path": "a.txt" })), Some(&root()), Some(&home()), &[]);
        let only_reject = vec![PermissionOption { option_id: "no".into(), name: "No".into(), kind: "reject_once".into() }];
        assert!(matches!(answer_from(&tool, &decision, &only_reject).decision, AutoPermission::Reject { .. }));
        assert!(matches!(fail_closed("db locked", &options()).decision, AutoPermission::Reject { option_id: Some(_) }));
    }
}
