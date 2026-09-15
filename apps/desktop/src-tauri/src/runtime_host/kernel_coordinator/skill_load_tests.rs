//! `skill_load` on the Kernel path: on-demand enablement is bounded by the
//! conversation's enabled skills and by the Run's frozen tool scope, and every
//! load is audited with the exact content version returned.

use super::*;
use crate::runtime_host::kernel_gateway::GatewayPolicy;
use serde_json::{json, Value};

const TOOL: &str = "skill_load";

fn write_skill(skills_dir: &std::path::Path, directory: &str, body: &str) {
    let directory = skills_dir.join(directory);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("SKILL.md"), body).unwrap();
}

fn scope_with(tool_names: &[&str]) -> crate::database::KernelHostScope {
    crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: tool_names.iter().map(|name| (*name).to_owned()).collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: vec![],
    }
}

/// A prepared Run with one settled dummy dispatch (so resource acquisition is
/// valid), frozen to `tool_names`. The model configuration proposes exactly the
/// tools the freeze may name, including `skill_load`.
#[allow(clippy::type_complexity)]
fn prepared_policy(
    tool_names: &[&str],
) -> (
    Database,
    PathBuf,
    String,
    GatewayPolicy,
    crate::kernel::CancellationToken,
) {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let mut config = super::worker_configuration();
    config.proposal_tools.push(json!({
        "name": TOOL,
        "description": "Load the full instructions of an enabled skill",
        "parameters": { "type": "object", "properties": { "skillId": { "type": "string" } },
                        "required": ["skillId"] },
    }));
    let (db, root, run_id) =
        super::fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    db.freeze_kernel_host_scope(&run_id, &scope_with(tool_names)).unwrap();
    cancellation.register_run(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_tools("batch", super::calls(), &Allow).unwrap();
    assert!(
        coordinator
            .dispatch_tool("read-a", "reader-owner", |_, _, _| {
                Ok((
                    true,
                    json!({"content":[{"type":"text","text":"ready"}],"details":{}}),
                ))
            })
            .unwrap(),
        "the Kernel dispatch must settle"
    );
    let binding = db.run_control_binding(&run_id).unwrap().unwrap();
    let scope = db.kernel_host_scope(&run_id).unwrap();
    let policy = GatewayPolicy { binding, scope };
    let token = cancellation.run_token(&run_id).unwrap();
    (db, root, run_id, policy, token)
}

#[test]
fn loads_enabled_skill_text_and_audits_the_exact_content_version() {
    let (db, root, run_id, policy, token) = prepared_policy(&["read", TOOL]);
    let skills_dir = root.join("skills");
    let body = "Late skill body 按需加载正文.";
    write_skill(
        &skills_dir,
        "late",
        &format!(
            "---\nid: late\nname: Late\nversion: 2.3\nrequired_tools: [read]\n---\n{body}"
        ),
    );
    db.set_agent_skill_enabled(&db.default_agent_id(), "late", true)
        .unwrap();

    let result = policy
        .execute_context_resource(
            &db,
            &root,
            &root,
            &skills_dir,
            TOOL,
            &json!({"skillId": "late"}),
            &token,
        )
        .expect("enabled skill must load");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(body), "{text}");
    assert!(text.starts_with("## Skill: Late (late)"));
    assert_eq!(result["details"]["skillId"], json!("late"));
    assert_eq!(result["details"]["version"], json!("2.3"));
    assert_eq!(result["details"]["source"], json!("on_demand"));
    assert_eq!(result["details"]["toolsAvailable"], json!(true));
    assert!(result["details"]["warning"].is_null());
    let hash = result["details"]["contentSha256"].as_str().unwrap();
    assert!(hash.starts_with("sha256:"));
    // Chinese content is counted in Unicode scalar values; bytes are reported
    // separately and stay larger for multibyte text.
    assert_eq!(
        result["details"]["chars"].as_u64().unwrap() as usize,
        body.chars().count()
    );
    assert!(
        result["details"]["bytes"].as_u64().unwrap()
            > result["details"]["chars"].as_u64().unwrap()
    );

    let activations = db.skill_activations(&run_id).unwrap();
    let only = activations
        .iter()
        .find(|row| row.skill_id == "late")
        .expect("activation audited");
    assert_eq!(only.source, "on_demand");
    assert_eq!(only.version, "2.3");
    assert_eq!(only.content_sha256, hash);
    assert!(only.tools_available);
    assert_eq!(only.char_count as usize, body.chars().count());
    assert_eq!(only.byte_count as usize, body.len());

    // Loading identical content again does not duplicate the audit row.
    policy
        .execute_context_resource(
            &db,
            &root,
            &root,
            &skills_dir,
            TOOL,
            &json!({"skillId": "late"}),
            &token,
        )
        .unwrap();
    assert_eq!(
        db.skill_activations(&run_id)
            .unwrap()
            .iter()
            .filter(|row| row.skill_id == "late")
            .count(),
        1
    );

    // A reopened database still sees the activation audit (no in-memory state).
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(
        db.skill_activations(&run_id)
            .unwrap()
            .iter()
            .filter(|row| row.skill_id == "late")
            .count(),
        1
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn missing_frozen_tools_are_reported_and_loading_never_grows_scope() {
    let (db, root, _run_id, policy, token) = prepared_policy(&["read", TOOL]);
    let skills_dir = root.join("skills");
    write_skill(
        &skills_dir,
        "writer",
        "---\nid: writer\nname: Writer\nversion: 1.0\nrequired_tools: [write_file]\n---\nWriter instructions.",
    );
    db.set_agent_skill_enabled(&db.default_agent_id(), "writer", true)
        .unwrap();

    let result = policy
        .execute_context_resource(
            &db,
            &root,
            &root,
            &skills_dir,
            TOOL,
            &json!({"skillId": "writer"}),
            &token,
        )
        .expect("text loads even when a dependency tool is frozen out");
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Writer instructions."));
    assert_eq!(result["details"]["toolsAvailable"], json!(false));
    assert_eq!(result["details"]["missingTools"], json!(["write_file"]));
    assert!(
        result["details"]["warning"]
            .as_str()
            .unwrap()
            .contains("冻结工具范围")
    );
    // The frozen scope is unchanged: the model still cannot call write_file,
    // proven through the same admission gate a real call reaches.
    assert!(!policy.scope.tool_names.contains("write_file"));
    let error = policy
        .execute_context_resource(
            &db,
            &root,
            &root,
            &skills_dir,
            "write_file",
            &json!({"path": "a.txt", "content": "x"}),
            &token,
        )
        .unwrap_err();
    assert!(
        error.contains("frozen Kernel resource catalog"),
        "unexpected refusal: {error}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn unknown_disabled_and_scope_shaped_skill_loads_are_rejected() {
    let (db, root, run_id, policy, token) = prepared_policy(&["read", TOOL]);
    let skills_dir = root.join("skills");
    write_skill(
        &skills_dir,
        "on",
        "---\nid: on\nname: On\nversion: 1.0\nrequired_tools: [read]\n---\nOn body.",
    );
    write_skill(
        &skills_dir,
        "off",
        "---\nid: off\nname: Off\nversion: 1.0\nrequired_tools: [read]\n---\nOff body.",
    );
    db.set_agent_skill_enabled(&db.default_agent_id(), "on", true)
        .unwrap();

    let call = |input: &Value| {
        policy.execute_context_resource(&db, &root, &root, &skills_dir, TOOL, input, &token)
    };
    // Enabled-but-unknown id and present-but-disabled id.
    let error = call(&json!({"skillId": "nope"})).unwrap_err();
    assert!(error.contains("未找到已启用的技能"), "{error}");
    let error = call(&json!({"skillId": "off"})).unwrap_err();
    assert!(error.contains("未在当前任务中启用"), "{error}");

    // The model may never name its own scope.
    for field in ["runId", "conversationId", "scope", "enabled", "grantTools"] {
        let mut input = json!({"skillId": "on"});
        input[field] = json!("foreign");
        let error = call(&input).unwrap_err();
        assert!(
            error.contains("is not an argument of skill_load"),
            "{field}: {error}"
        );
    }
    // Path-shaped and empty ids are rejected before touching the filesystem.
    assert!(call(&json!({"skillId": "../on"})).is_err());
    assert!(call(&json!({"skillId": ""})).is_err());
    // Loading and listing are separate modes, never one ambiguous call.
    assert!(call(&json!({"skillId": "on", "query": "x"})).is_err());
    assert!(call(&json!({"skillId": "on", "offset": 3})).is_err());
    assert!(call(&json!({"query": "x".repeat(200)})).is_err());
    assert!(call(&json!({"query": false})).is_ok());

    // N7: a skill that the prompt could not list anywhere is still discoverable
    // by id through the model-facing tool, and that discovery grants nothing.
    let discover = call(&json!({})).expect("a bare catalog query is the way in");
    let entries = discover["details"]["entries"].as_array().unwrap();
    assert_eq!(discover["details"]["mode"], json!("catalog"));
    assert!(
        entries.iter().any(|entry| entry["skillId"] == json!("on")),
        "the enabled skill must be discoverable: {discover}"
    );
    assert!(
        !entries.iter().any(|entry| entry["skillId"] == json!("off")),
        "a disabled skill may never be listed: {discover}"
    );
    // Discovery injects no instructions and records no activation.
    assert!(
        discover["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("skill_load"),
        "the page must say how to load"
    );
    assert!(db.skill_activations(&run_id).unwrap().is_empty());
    // The id from the page then loads the full text.
    let page = call(&json!({})).unwrap();
    let discovered_id = page["details"]["entries"][0]["skillId"]
        .as_str()
        .unwrap()
        .to_owned();
    let loaded = call(&json!({"skillId": discovered_id})).unwrap();
    assert_eq!(loaded["details"]["mode"], json!("load"));
    assert!(loaded["content"][0]["text"].as_str().unwrap().contains("On body."));
    // Search narrows by name/description, and paging is bounded and resumable.
    let searched = call(&json!({"query": "On"})).unwrap();
    assert_eq!(searched["details"]["totalMatched"], json!(1));
    assert!(searched["details"]["nextOffset"].is_null());
    let limited = call(&json!({"limit": 1})).unwrap();
    assert_eq!(limited["details"]["returned"], json!(1));
    assert!(limited["details"]["nextOffset"].is_null() || limited["details"]["nextOffset"].is_number());

    // A frozen catalogue that omits skill_load is refused at admission (validate
    // runs before the stored-scope identity check), before any filesystem read.
    let narrowed = GatewayPolicy {
        binding: policy.binding.clone(),
        scope: scope_with(&["read"]),
    };
    let error = narrowed
        .execute_context_resource(
            &db,
            &root,
            &root,
            &skills_dir,
            TOOL,
            &json!({"skillId": "on"}),
            &token,
        )
        .unwrap_err();
    // validate() runs before the identity comparison, so the frozen catalogue
    // refusal is what the model sees.
    assert!(
        error.contains("frozen Kernel resource catalog"),
        "unexpected refusal: {error}"
    );
    let _ = std::fs::remove_dir_all(root);
}
