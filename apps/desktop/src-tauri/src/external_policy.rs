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
use fox_agent_backend::port::AgentBackend;
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
        // Plan C3: the Fox tool extension's tools. The gate only lets them
        // reach the Fox tool bridge, which makes the real decision (the same
        // engine, `external_tools`), so the gate itself treats them as reads.
        tool if fox_agent_backend::fox_mcp::TOOL_NAMES.contains(&tool) => (approval.tool_name.clone(), ToolSubject::Read),
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
/// rules as they are now. Every automatic allow of a non-read call, and every
/// block, leaves one row in the same audit as PiNative.
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
    if (decision.is_allow() && decision.class != ToolClass::Read) || decision.is_block() {
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

/// `toolCall.rawInput.command`：字符串，或字符串数组（按空格拼接，`["bash","-lc","脚本"]` 交给嵌套分析）。
fn command_line(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        let text = text.trim();
        return (!text.is_empty()).then(|| text.to_owned());
    }
    let words: Vec<&str> = value.as_array()?.iter().filter_map(Value::as_str).map(str::trim).filter(|word| !word.is_empty()).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Codex / dsh 的 `session/request_permission` 里 Fox 能看到命令时，只做 block 判定。
/// 没有命令、或没有命中 block 时返回 `None`（不替它们放行）。
pub fn native_block_decision(
    backend: AgentBackend,
    native_mode: &str,
    request: &Value,
    project_root: Option<&Path>,
    home: Option<&Path>,
    rules: &[PolicyRule],
) -> Option<PolicyDecision> {
    let raw = request.get("toolCall")?.get("rawInput")?;
    let command = command_line(raw.get("command")?)?;
    let cwd = raw
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .or_else(|| project_root.map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let subject = ToolSubject::Command { command, cwd, model_risk: None };
    let decision = approval_policy::decide(&approval_policy::PolicyRequest {
        tool: "run_command",
        subject: &subject,
        mode: crate::external_tools::external_access_mode(backend, native_mode),
        project_root,
        home,
        rules,
    });
    decision.is_block().then_some(decision)
}

/// Codex / dsh：命中 block 时拒绝并记审计。取不到命令返回 `Ok(None)`；数据库出错返回 `Err`（调用方 fail-closed）。
pub fn block_for_native_request(
    database: &crate::database::Database,
    run_id: &str,
    conversation_id: &str,
    backend: AgentBackend,
    native_mode: &str,
    request: &Value,
    options: &[PermissionOption],
) -> Result<Option<HostPermissionAnswer>, String> {
    let context = database.external_conversation_context(conversation_id)?.ok_or_else(|| "conversation not found".to_owned())?;
    let rules = database.policy_rules_for_conversation(conversation_id)?;
    let root = context.project_root.as_deref().map(|root| {
        let root = PathBuf::from(root);
        root.canonicalize().unwrap_or(root)
    });
    let home = approval_policy::host_home();
    let Some(decision) = native_block_decision(backend, native_mode, request, root.as_deref(), home.as_deref(), &rules) else {
        return Ok(None);
    };
    let tool = "run_command";
    let raw = request.get("toolCall").and_then(|call| call.get("rawInput"));
    let summary = raw.and_then(|raw| raw.get("command")).and_then(command_line).unwrap_or_else(|| tool.to_owned());
    let tool_call_id = request.get("toolCall").and_then(|call| call.get("toolCallId")).and_then(Value::as_str).filter(|id| !id.is_empty());
    database.record_permission_audit(conversation_id, Some(run_id), tool_call_id, tool, &decision, &summary)?;
    Ok(Some(answer_from(tool, &decision, options)))
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
    fn fox_bridge_tools_pass_the_pi_gate_as_reads_in_every_tier() {
        for mode in [PI_READ_ONLY, "ask", PI_ALLOW, PI_FULL] {
            for tool in fox_agent_backend::fox_mcp::TOOL_NAMES {
                assert_eq!(verdict(mode, &gate(tool, json!({})), &[]), "allow", "{tool} in {mode}");
            }
        }
        assert_eq!(verdict("ask", &gate("some_other_extension_tool", json!({})), &[]), "ask", "other extension tools are not widened");
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

    #[test]
    fn pi_gate_blocks_destructive_commands_in_full_access_and_allows_dir() {
        let win = PathBuf::from("D:/fox-test/测试项目");
        let win_home = PathBuf::from("C:/Users/fox");
        for command in [
            r"rmdir /s /q D:\fox-test\测试项目",
            r#"cmd //c "rmdir /s /q D:\\fox-test\\测试项目""#,
            "rm -rf /d/fox-test/测试项目",
            r#"powershell -Command "Remove-Item -Recurse -Force 'D:\fox-test\测试项目'""#,
            "rm -rf /c/Windows/System32",
        ] {
            let request = gate("bash", json!({ "command": command }));
            let (tool, decision) = decide_pi(PI_FULL, &request, Some(&win), Some(&win_home), &[]);
            assert!(decision.is_block(), "{command}: {:?}", decision.verdict);
            let answer = answer_from(&tool, &decision, &options());
            assert!(matches!(answer.decision, AutoPermission::Reject { .. }), "{command}");
            assert_eq!(answer.policy["denyCode"], "permission.blocked", "{command}");
            let findings = answer.policy["findings"].as_array().unwrap();
            assert!(findings.iter().any(|finding| finding == "builtin_block"), "{command}: {findings:?}");
        }
        assert_eq!(verdict(PI_ALLOW, &gate("bash", json!({ "command": "dir" })), &[]), "allow");
    }

    #[test]
    fn native_block_decision_rejects_codex_root_deletes_and_ignores_npm_test() {
        use fox_agent_backend::port::AgentBackend;
        let project = root();
        let home_dir = home();
        let root_text = project.display().to_string();
        let string_request = json!({
            "toolCall": { "toolCallId": "t1", "rawInput": { "command": format!("rm -rf {root_text}"), "cwd": &root_text } }
        });
        let decision = native_block_decision(AgentBackend::Codex, "agent-full-access", &string_request, Some(&project), Some(&home_dir), &[])
            .expect("string command is a block");
        assert!(decision.is_block());
        assert!(matches!(decision.verdict, crate::runtime_host::approval_policy::Verdict::Deny { ref code, .. } if code == "permission.blocked"));
        let array_request = json!({
            "toolCall": { "rawInput": { "command": ["bash", "-lc", format!("rm -rf {root_text}")] } }
        });
        let decision = native_block_decision(AgentBackend::Deepseek, "workspace-write", &array_request, Some(&project), Some(&home_dir), &[])
            .expect("bash -lc array is a block");
        assert!(decision.is_block());
        let npm = json!({ "toolCall": { "rawInput": { "command": "npm test", "cwd": &root_text } } });
        assert!(native_block_decision(AgentBackend::Codex, "workspace-write", &npm, Some(&project), Some(&home_dir), &[]).is_none());
        assert!(native_block_decision(AgentBackend::Codex, "workspace-write", &json!({ "toolCall": { "rawInput": {} } }), Some(&project), Some(&home_dir), &[]).is_none());
    }
}
