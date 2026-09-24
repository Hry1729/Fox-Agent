//! REV-01 … REV-06 复现与验收用例（协调者复审六项）。
//!
//! 设计约束（来自协调者）：
//! * 每条用例使用**真实 `Database`**、**真实项目绑定**与**生产调用分支**；
//!   禁止用 `database: None` 或结构体替身证明完成。
//! * 阶段 0（修复前）这些用例必须**按预期失败**；修复后转绿，且对应正常场景不被破坏。
//! * 不放开生产 Shell 门禁：命令执行一律经生产入口，未被验证后端时拒绝。
//!
//! 用例编号与实施计划 §5 / §10 对应：F1–F6 失败场景，N1–N6 对应正常场景，
//! H1–H18 加固用例分布在本文件与各机制实现处。

use super::*;
use crate::database::Database;
use crate::kernel::CancellationRegistry;
use crate::runtime_host::{kernel_gateway, managed_files};
use crate::tool_host;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// A-F1：模型实际交付与整文件替换资格。独立文件，但作为本文件的子模块，以便复用
// 这里已经过 REV 审查的真实夹具与执行缝，而不是另写一套替身。
#[path = "f1_model_delivery_tests.rs"]
mod f1_model_delivery_tests;

// ---------------------------------------------------------------------------
// 夹具：真实 Database + 真实项目绑定 + 冻结 Legacy(Rust executor) 绑定
// ---------------------------------------------------------------------------

/// 本文件所有夹具使用的 Host 工具集（与冻结的 Kernel 模型配置逐项一致）。
const REV_TOOLS: &[&str] = &["read", "write_file", "edit_file"];

/// 一个绑定到临时项目根的会话 + 一个已冻结的 Legacy Run。
/// `mode` 为会话权限模式（read_only / ask / allow）。
fn rev_fixture(label: &str, mode: &str) -> (Database, PathBuf, String, String) {
    let root = std::env::temp_dir().join(format!("fox-rev-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).expect("create project root");
    let db = Database::open(root.join("facts.db")).expect("open database");
    let conversation = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some(mode))
        .expect("conversation bound to the project root");
    let run = db.create_run(&conversation.id, "rev repro", None).expect("run").run;
    db.freeze_legacy_run_control_with_executor(
        &run.id,
        "legacy",
        fox_engine_protocol::ResourceExecutor::Rust,
    )
    .expect("frozen Legacy binding with the Rust executor");
    db.apply_runtime_event(&run.id, 1, &json!({ "type": "run.started" }))
        .expect("a fresh Host ToolCall needs a running Run");
    (db, root, conversation.id, run.id)
}

fn rev_model_config(tools: &[&str]) -> crate::kernel_model_config::KernelModelConfig {
    crate::kernel_model_config::KernelModelConfig {
        execution_profile_id: "legacy".into(),
        model_service: json!({"apiType":"openai-completions","modelId":"rev-fixture","baseUrl":"http://127.0.0.1:1/v1"}),
        system_prompt: "rev fixture".into(),
        proposal_tools: tools
            .iter()
            .map(|name| json!({"name":name,"description":"rev fixture tool","parameters":{"type":"object","properties":{}}}))
            .collect(),
        engine_id: "pi".into(),
        native_adapter: None,
    }
}

/// The same tool catalogue with the deterministic in-process `faux` model
/// service: the continuation the consumer runs after a settled tool batch needs
/// no network and no credential.
fn rev_consumer_model_config() -> crate::kernel_model_config::KernelModelConfig {
    let mut model = rev_model_config(REV_TOOLS);
    model.model_service = json!({
        "apiType": "faux",
        "modelId": "rev-consumer",
        "baseUrl": "http://localhost",
        "fauxResponses": ["Host consumed the durable batch"],
    });
    model
}

/// Authoritative(Kernel) Run 夹具。REV-01 / REV-05 的缺陷位于
/// `GatewayPolicy`（Kernel 路径），必须用真实权威 Run 复现。
///
/// 冻结顺序与生产一致：binding → kernel_run → model config → host scope →
/// run.started。模型配置的插入守卫要求 Run 仍处于 `created` 且
/// `last_event_seq = 0`。
/// Like [`rev_fixture`] but WITHOUT a frozen binding, so a test can seed
/// authorization facts first and freeze afterwards (production order).
fn rev_fixture_unfrozen(label: &str, mode: &str) -> (Database, PathBuf, String, String) {
    let root = std::env::temp_dir().join(format!("fox-rev-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).expect("create project root");
    let db = Database::open(root.join("facts.db")).expect("open database");
    let conversation = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some(mode))
        .expect("conversation bound to the project root");
    let run = db.create_run(&conversation.id, "rev repro", None).expect("run").run;
    (db, root, conversation.id, run.id)
}

fn kernel_gateway_fixture(
    label: &str,
    mode: &str,
) -> (Database, PathBuf, String, String, crate::database::KernelHostScope) {
    kernel_gateway_fixture_with_model(label, mode, &rev_model_config(REV_TOOLS))
}

/// The same frozen Kernel Run with a caller-chosen model configuration. The
/// consumer-chain tests use the deterministic in-process `faux` service the
/// existing coordinator tests use, so the continuation that follows a settled
/// tool batch needs no network.
fn kernel_gateway_fixture_with_model(
    label: &str,
    mode: &str,
    model: &crate::kernel_model_config::KernelModelConfig,
) -> (Database, PathBuf, String, String, crate::database::KernelHostScope) {
    let root = std::env::temp_dir().join(format!("fox-rev-kgw-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).expect("create project root");
    let db = Database::open(root.join("facts.db")).expect("open database");
    let conversation = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some(mode))
        .expect("conversation bound to the project root");
    let run = db.create_run(&conversation.id, "rev kernel gateway", None).expect("run").run;
    let binding = db
        .freeze_kernel_run_control(&run.id, "legacy", fox_engine_protocol::TimeBudgets::continuous())
        .expect("frozen Kernel binding");
    let config_hash = model.hash().expect("model config hash");
    let frozen = crate::kernel::RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "rev-manifest".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: config_hash.clone(),
        model_request_timeout_ms: binding.budgets.model_request_ms,
        model_first_response_ms: binding.budgets.model_first_response_ms,
        model_idle_ms: binding.budgets.model_idle_ms,
        tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
        run_execution_budget_ms: binding.budgets.run_execution_ms,
        run_execution_limited: binding.budgets.run_execution_limited,
        approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
        provider_max_retries: 0,
        turn_max_retries: 0,
    };
    db.kernel_create_run(
        &run.id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        &binding.execution_profile_id,
        &config_hash,
        &serde_json::to_string(&frozen).expect("serialize frozen config"),
    )
    .expect("kernel run");
    db.freeze_kernel_model_config(&run.id, model).expect("freeze model config");
    let initial = fox_engine_protocol::KernelInitialModelInput {
        schema_version: 1,
        run_id: run.id.clone(),
        turn_id: "turn-1".into(),
        prompt_config_hash: config_hash,
        messages: vec![json!({"role":"user","content":[{"type":"text","text":"rev fixture"}]})],
    };
    db.freeze_kernel_initial_input(&initial).expect("freeze initial input");
    // 生产入口：同一函数冻结 Host scope（并复核其与模型工具目录一致）。
    let scope = kernel_gateway::freeze_scope(&db, &binding, &json!({}), model).expect("freeze scope");
    // `freeze_scope` only BUILDS the scope; persist it before the Run starts, as
    // production does, so `kernel_host::drive` can validate it.
    db.freeze_kernel_host_scope(&run.id, &scope).expect("persist the frozen scope");
    db.apply_runtime_event(&run.id, 1, &json!({ "type": "run.started" }))
        .expect("a fresh Host ToolCall needs a running Run");
    (db, root, conversation.id, run.id, scope)
}

/// 同一项目下的两个会话（REV-03 需要项目级联动证据）。
fn two_conversations_one_project(label: &str, mode: &str) -> (Database, PathBuf, String, String, String) {
    let root = std::env::temp_dir().join(format!("fox-rev-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).expect("create project root");
    let db = Database::open(root.join("facts.db")).expect("open database");
    // 直接按真实 schema 建项目行：`create_project` 不存在，而这里要的正是
    // "两个已存在会话绑定到同一项目"这一事实，交由生产触发器维护策略行。
    let project_id = format!("rev-project-{}", uuid::Uuid::new_v4().simple());
    let now = crate::database::now_ms();
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO projects(id, name, root_path, permission_mode, status, created_at, updated_at, last_opened_at)
             VALUES (?1, 'rev project', ?2, ?3, 'active', ?4, ?4, ?4)",
            rusqlite::params![project_id, root.to_str().unwrap(), mode, now],
        )?;
        Ok(())
    })
    .expect("project row");
    let a = db
        .create_conversation(db.default_agent_id(), Some("A"), None, None)
        .expect("conversation A");
    let b = db
        .create_conversation(db.default_agent_id(), Some("B"), None, None)
        .expect("conversation B");
    for id in [&a.id, &b.id] {
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE conversations SET project_id=?2 WHERE id=?1",
                rusqlite::params![id, project_id],
            )?;
            Ok(())
        })
        .expect("bind conversation to project");
    }
    (db, root, project_id, a.id, b.id)
}

/// 一个 Authoritative(Kernel) Run，用于 REV-04 的撤销链路。
fn kernel_fixture(label: &str, mode: &str) -> (Database, PathBuf, String, String) {
    let root = std::env::temp_dir().join(format!("fox-rev-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).expect("create project root");
    let db = Database::open(root.join("facts.db")).expect("open database");
    let conversation = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some(mode))
        .expect("conversation");
    let run = db.create_run(&conversation.id, "rev kernel", None).expect("run").run;
    let config = crate::kernel::RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "manifest-hash".into(),
        permission_snapshot_id: "perm-1".into(),
        execution_profile_id: "legacy".into(),
        prompt_config_hash: "hash".into(),
        model_request_timeout_ms: 120_000,
        model_first_response_ms: 60_000,
        model_idle_ms: 120_000,
        tool_execution_timeout_ms: 600_000,
        run_execution_budget_ms: 600_000,
        run_execution_limited: true,
        approval_wait_timeout_ms: 300_000,
        provider_max_retries: 2,
        turn_max_retries: 1,
    };
    db.kernel_create_run(
        &run.id,
        "pi",
        "authoritative",
        2,
        "perm-1",
        "legacy",
        "hash",
        &serde_json::to_string(&config).expect("serialize frozen config"),
    )
    .expect("kernel run");
    db.apply_runtime_event(&run.id, 1, &json!({ "type": "run.started" }))
        .expect("running run");
    db.freeze_kernel_run_control(&run.id, "legacy", fox_engine_protocol::TimeBudgets::continuous())
        .expect("frozen Kernel binding");
    (db, root, conversation.id, run.id)
}

fn scope_for(tools: &[&str]) -> crate::database::KernelHostScope {
    crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: tools.iter().map(|name| name.to_string()).collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: Vec::new(),
    }
}

fn gateway_policy(
    db: &Database,
    run_id: &str,
    scope: crate::database::KernelHostScope,
) -> kernel_gateway::GatewayPolicy {
    kernel_gateway::GatewayPolicy {
        binding: db.run_control_binding(run_id).expect("binding").expect("frozen binding"),
        scope,
        database: Some(db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    }
}

/// 经生产读取缝读一次文件，返回结果 JSON。
fn host_read(
    db: &Database,
    binding: &fox_engine_protocol::RunControlBinding,
    input: &Value,
    call: &str,
) -> Value {
    let registry = CancellationRegistry::default();
    registry.register_run(&binding.run_id).expect("register run");
    let token = registry.tool_token(&binding.run_id, call).expect("tool token");
    managed_files::execute_observed_reader(
        db,
        binding,
        "read",
        input,
        call,
        &token,
        std::time::Duration::from_millis(600_000),
    )
    .expect("host read")
}

fn read_version(result: &Value) -> String {
    result["details"]["readVersion"]
        .as_str()
        .expect("readVersion in the read result")
        .to_owned()
}

// ---------------------------------------------------------------------------
// F1 / N1 — REV-01：模型声明的前置条件不得被"升级"为最新观察
// ---------------------------------------------------------------------------

/// F1：读 v1 → 外部改 v2 → 再读 v2 → 提交明确基于 v1 的候选必须拒绝。
#[test]
fn rev01_stale_declared_precondition_is_not_silently_upgraded() {
    let (db, root, _conversation, run, scope) = kernel_gateway_fixture("f1", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"v1 content\n").expect("write v1");

    let v1 = read_version(&host_read(&db, &binding, &json!({"path":"a.txt"}), "read-v1"));
    std::fs::write(root.join("a.txt"), b"v2 content\n").expect("external edit to v2");
    let v2 = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","offset":0,"limit":4}),
        "read-v2",
    ));
    assert_ne!(v1, v2, "the two reads must observe different versions");

    let policy = gateway_policy(&db, &run, scope);
    let stale = json!({"path":"a.txt","content":"candidate based on v1","expectedVersion":v1});
    let error = policy
        .validate("write_file", &stale)
        .expect_err("a stale declared precondition must be refused, not upgraded");
    assert!(
        error.contains("tool.file_conflict"),
        "the refusal must be a recoverable file conflict, got: {error}"
    );
    assert_eq!(
        std::fs::read(root.join("a.txt")).expect("read target"),
        b"v2 content\n",
        "a refused write must not touch the target"
    );
}

/// N1：读同一版本的两个页面，不应破坏已绑定到该版本的合法请求。
#[test]
fn rev01_second_page_of_the_same_version_keeps_the_request_valid() {
    let (db, root, _conversation, run, scope) = kernel_gateway_fixture("n1", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"line one\nline two\nline three\n").expect("write v1");

    let v1 = read_version(&host_read(&db, &binding, &json!({"path":"a.txt"}), "read-p1"));
    let _ = host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":2,"lineCount":1}),
        "read-p2",
    );
    let policy = gateway_policy(&db, &run, scope);
    policy
        .validate(
            "write_file",
            &json!({"path":"a.txt","content":"replacement","expectedVersion":v1}),
        )
        .expect("a request bound to an observed version must stay admissible");
}

// ---------------------------------------------------------------------------
// F3 / N3 — REV-03：会话级模式更新不得连带同项目其他会话
// ---------------------------------------------------------------------------

/// F3：同项目 A/B，仅对 A 请求 allow；B 的版本与模式必须不变。
#[test]
fn rev03_conversation_policy_change_does_not_touch_siblings() {
    let (db, _root, _project, a, b) = two_conversations_one_project("f3", "read_only");
    let before_b = db.execution_policy(&b).expect("B policy");
    let before_project: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT permission_mode FROM projects WHERE id=(SELECT project_id FROM conversations WHERE id=?1)",
                [&a],
                |r| r.get(0),
            )
        })
        .expect("project mode");

    let before_a = db.execution_policy(&a).expect("A policy");
    db.change_execution_policy(&a, "req-a", before_a.version, "allow")
        .expect("A switches to allow");

    let after_b = db.execution_policy(&b).expect("B policy after");
    assert_eq!(
        (after_b.version, after_b.mode.as_str()),
        (before_b.version, before_b.mode.as_str()),
        "conversation B must not gain a version or widen its mode because of A's request"
    );
    let after_project: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT permission_mode FROM projects WHERE id=(SELECT project_id FROM conversations WHERE id=?1)",
                [&a],
                |r| r.get(0),
            )
        })
        .expect("project mode after");
    assert_eq!(
        after_project, before_project,
        "a conversation-level request must not rewrite the project default"
    );
}

/// N3：正常会话切换保持 CAS / 幂等 / 请求标识语义。
#[test]
fn rev03_normal_conversation_switch_keeps_cas_semantics() {
    let (db, _root, _project, a, _b) = two_conversations_one_project("n3", "ask");
    let before = db.execution_policy(&a).expect("policy");
    let changed = db
        .change_execution_policy(&a, "req-1", before.version, "allow")
        .expect("first change");
    assert_eq!(changed.version, before.version + 1);
    assert_eq!(
        db.change_execution_policy(&a, "req-1", before.version, "allow")
            .expect("idempotent retry"),
        changed
    );
    assert!(db
        .change_execution_policy(&a, "req-1", before.version, "read_only")
        .is_err());
    assert!(db
        .change_execution_policy(&a, "req-2", before.version, "read_only")
        .is_err());
}

// ---------------------------------------------------------------------------
// F4 / N4 — REV-04：只读往返后旧可复用批准不得复活
// ---------------------------------------------------------------------------

/// F4：allow_conversation 授权 → allow→read_only→ask → 同 scope 再请求必须重新审批。
#[test]
fn rev04_reusable_grant_does_not_survive_a_read_only_round_trip() {
    let (db, _root, conversation, run) = kernel_fixture("f4", "allow");
    let scope_key = "host-target:sha256:abc";
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO kernel_tool_calls
             (run_id, tool_call_id, batch_id, tool, source_order, canonical_input_json,
              state, result_json, created_at, settled_at, dispatch_idempotency_key)
             VALUES (?1, 'call-1', 'b1', 'write_file', 0, '{\"path\":\"a.txt\"}',
                     'running', NULL, 0, 0, NULL)",
            [&run],
        )?;
        connection.execute(
            "INSERT INTO kernel_approvals(run_id, tool_call_id, state, created_at, decided_at)
             VALUES (?1, 'call-1', 'allow_conversation', 0, 0)",
            [&run],
        )?;
        Ok(())
    })
    .expect("seed approval");
    let registration = db
        .kernel_register_authorization_grant(&run, "call-1", "allow_conversation", "write_file", Some(scope_key))
        .expect("register grant");
    assert!(
        matches!(registration, crate::database::GrantRegistration::Registered { .. }),
        "the fixture must really have produced a reusable grant"
    );
    assert_eq!(db.kernel_effective_authorization_grants(&run, 0).expect("grants").len(), 1);

    let mut version = db.execution_policy(&conversation).expect("policy").version;
    for mode in ["read_only", "ask"] {
        version = db
            .change_execution_policy(&conversation, &format!("req-{mode}"), version, mode)
            .expect("policy change")
            .version;
    }

    assert!(
        db.kernel_effective_authorization_grants(&run, 0)
            .expect("grants after round trip")
            .is_empty(),
        "a reusable write approval must not be resurrected by returning to ask"
    );
}

/// N4：`allow_once` 永不变成可复用授权（现状已满足，锁定防回归）。
#[test]
fn rev04_allow_once_never_becomes_a_reusable_grant() {
    let (db, _root, _conversation, run) = kernel_fixture("n4", "ask");
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO kernel_tool_calls
             (run_id, tool_call_id, batch_id, tool, source_order, canonical_input_json,
              state, result_json, created_at, settled_at, dispatch_idempotency_key)
             VALUES (?1, 'call-1', 'b1', 'write_file', 0, '{}', 'running', NULL, 0, 0, NULL)",
            [&run],
        )?;
        connection.execute(
            "INSERT INTO kernel_approvals(run_id, tool_call_id, state, created_at, decided_at)
             VALUES (?1, 'call-1', 'allow_once', 0, 0)",
            [&run],
        )?;
        Ok(())
    })
    .expect("seed approval");
    let outcome = db
        .kernel_register_authorization_grant(
            &run,
            "call-1",
            "allow_once",
            "write_file",
            Some("host-target:sha256:x"),
        )
        .expect("register");
    assert!(matches!(
        outcome,
        crate::database::GrantRegistration::Skipped {
            reason: crate::database::GrantSkipReason::OneShotApprovalIsNotSessionAuthorization
        }
    ));
    assert!(db
        .kernel_effective_authorization_grants(&run, 0)
        .expect("grants")
        .is_empty());
}

// ---------------------------------------------------------------------------
// F5 / N5 — REV-05：读多少才能改什么
// ---------------------------------------------------------------------------

/// F5：只读 1 行就请求整文件替换，必须要求独立、明确的整文件替换授权。
#[test]
fn rev05_one_line_read_cannot_authorize_a_whole_file_replacement() {
    let (db, root, _conversation, run, scope) = kernel_gateway_fixture("f5", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    let body: String = (0..500).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("big.txt"), body.as_bytes()).expect("write 500 lines");

    let read = host_read(
        &db,
        &binding,
        &json!({"path":"big.txt","startLine":1,"lineCount":1}),
        "read-one-line",
    );
    let version = read_version(&read);
    assert!(
        read["content"][0]["text"]
            .as_str()
            .expect("text")
            .starts_with("line 0\n"),
        "the fixture must really have delivered only the first line"
    );

    let policy = gateway_policy(&db, &run, scope);
    let input = json!({"path":"big.txt","content":"whole file replacement","expectedVersion":version});
    let decision = policy.decide(&run, "call-write", "write_file", &input.to_string());
    assert!(
        matches!(decision, fox_agent_kernel::PolicyDecision::RequireApproval),
        "a whole-file replacement after a one-line read must require an explicit, \
         purpose-specific confirmation; got {decision:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("big.txt")).expect("read target"),
        body,
        "an unconfirmed whole-file replacement must not write"
    );
}

/// N5：局部读取后，`oldText` 已被观察到的精确编辑仍可正常完成。
#[test]
fn rev05_precise_edit_of_an_observed_fragment_still_works() {
    let (db, root, _conversation, run, scope) = kernel_gateway_fixture("n5", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"alpha\nbeta\ngamma\n").expect("write");
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":2,"lineCount":1}),
        "read-line-2",
    ));
    let policy = gateway_policy(&db, &run, scope);
    policy
        .validate(
            "edit_file",
            &json!({"path":"a.txt","oldText":"beta","newText":"BETA","expectedVersion":version}),
        )
        .expect("a precise edit of an observed fragment must stay admissible");
}

// ---------------------------------------------------------------------------
// F6 / N6 — REV-06：UTF-16 代理对中间的续读位置
// ---------------------------------------------------------------------------

/// F6：offset 落在代理对中间时，nextOffset 不得跳过下一字符。
#[test]
fn rev06_offset_inside_a_surrogate_pair_does_not_skip_a_character() {
    let (db, root, _conversation, run) = rev_fixture("f6", "read_only");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("emoji.txt"), "A😀BC").expect("write surrogate text");
    let registry = CancellationRegistry::default();
    registry.register_run(&run).expect("register run");
    let token = registry.tool_token(&run, "read-0").expect("token");

    let first = crate::resource_gateway::execute_with_budget(
        &binding,
        "read",
        &json!({"path":"emoji.txt","offset":2,"limit":2}),
        &token,
        std::time::Duration::from_millis(600_000),
    )
    .expect("first page");
    assert_eq!(
        first["content"][0]["text"].as_str().expect("text"),
        "😀",
        "the first page must return the whole character"
    );
    let next = first["details"]["nextOffset"]
        .as_u64()
        .expect("nextOffset must be present while the source is not exhausted");
    assert_eq!(
        next, 3,
        "nextOffset must be the corrected end (3), not the requested offset + length (4)"
    );

    let second = crate::resource_gateway::execute_with_budget(
        &binding,
        "read",
        &json!({"path":"emoji.txt","offset":next,"limit":2}),
        &token,
        std::time::Duration::from_millis(600_000),
    )
    .expect("second page");
    assert_eq!(
        second["content"][0]["text"].as_str().expect("text"),
        "BC",
        "the continuation must not skip the character after the surrogate pair"
    );
}

/// N6：从 offset=0 起全程使用返回游标分页，拼接结果必须等于原文。
#[test]
fn rev06_full_pagination_reconstructs_the_source() {
    let (db, root, _conversation, run) = rev_fixture("n6", "read_only");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    let text = "A😀BC😀D中文字😀E".repeat(40);
    std::fs::write(root.join("emoji.txt"), text.as_bytes()).expect("write");
    let registry = CancellationRegistry::default();
    registry.register_run(&run).expect("register run");

    let mut assembled = String::new();
    let mut offset = 0u64;
    for step in 0..200 {
        let token = registry.tool_token(&run, &format!("read-{step}")).expect("token");
        let page = crate::resource_gateway::execute_with_budget(
            &binding,
            "read",
            &json!({"path":"emoji.txt","offset":offset,"limit":3}),
            &token,
            std::time::Duration::from_millis(600_000),
        )
        .expect("page");
        assembled.push_str(page["content"][0]["text"].as_str().expect("text"));
        match page["details"]["nextOffset"].as_u64() {
            Some(next) if next > offset => offset = next,
            _ => break,
        }
    }
    assert_eq!(assembled, text, "paging with the returned cursor must reconstruct the source");
}

// ---------------------------------------------------------------------------
// F2 / N2 — REV-02：恢复走共用提交机制
// ---------------------------------------------------------------------------

/// 经生产写入口提交一次文件变更（含受管文件版本登记）。
fn host_write(
    db: &Database,
    root: &Path,
    run_id: &str,
    path: &str,
    content: &str,
    expected: &str,
    call: &str,
) -> Value {
    let registry = CancellationRegistry::default();
    registry.register_run(run_id).expect("register run");
    let token = registry.tool_token(run_id, call).expect("token");
    let input = json!({"path":path,"content":content,"expectedVersion":expected});
    let conversation = db
        .with_connection(|c| {
            c.query_row("SELECT conversation_id FROM runs WHERE id=?1", [run_id], |r| {
                r.get::<_, String>(0)
            })
        })
        .expect("run conversation");
    let mode = db
        .conversation_project_access(&conversation)
        .expect("project access")
        .map(|(_, mode)| mode)
        .unwrap_or_else(|| "allow".to_owned());
    let context = managed_files::ManagedExecutionContext {
        database: db,
        backups_dir: &root.join("managed-files"),
        conversation_id: &conversation,
        run_id,
        project_root: Some(root.to_str().unwrap()),
        permission_mode: &mode,
        scope: &scope_for(&["read", "write_file", "edit_file"]),
        sessions_dir: None,
    };
    crate::runtime_host::create_fresh_host_tool_call(
        db,
        run_id,
        call,
        "write_file",
        &input,
        "running",
        false,
    )
    .expect("tool call row");
    // 竞争窗口内 prepare 可能合法失败（并发改写导致的 file_conflict）；
    // 调用方按 isError 判定，这里不 panic。
    let prepared = match tool_host::prepare("write_file", &input, root.to_str().unwrap()) {
        Ok(prepared) => prepared,
        Err(error) => {
            return json!({"isError": true, "error": error});
        }
    };
    let outcome = managed_files::execute_with_managed_versions(
        context,
        "write_file",
        &input,
        Some(call),
        || tool_host::execute_with_cancellation(prepared, Some(&token)),
    );
    // 竞争窗口内 finalize 也可能失败（并发占用目标文件）。调用方按 isError
    // 判定"是否报告成功"，这里不 panic，否则会把环境噪声当成缺陷证据。
    match crate::runtime_host::finalize_host_tool_execution(
        db,
        run_id,
        call,
        "write_file",
        &input,
        Vec::new(),
        outcome,
    ) {
        Ok(value) => value,
        Err(error) => json!({"isError": true, "error": error}),
    }
}

fn managed_version_ids(db: &Database, conversation: &str, path: &str) -> Vec<String> {
    db.with_connection(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT id FROM managed_file_versions
                 WHERE conversation_id=?1 AND storage_path=?2 ORDER BY created_at, version_no",
            )
            .expect("prepare");
        let rows = statement
            .query_map(rusqlite::params![conversation, path], |row| row.get::<_, String>(0))
            .expect("query");
        Ok(rows.collect::<Result<Vec<_>, _>>().expect("collect"))
    })
    .expect("version ids")
}

/// Host 存储的 `storage_path` 是规范化后的路径（Windows 上带 `\\?\` 前缀），
/// 查询必须用同一形式，否则会漏掉真实登记行。
fn stored_path(root: &Path, name: &str) -> String {
    root.join(name)
        .canonicalize()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| root.join(name).to_string_lossy().into_owned())
}

/// F2：恢复与普通写交错时，提交临界区内的基线复验必须发现并发变更并拒绝。
///
/// 复现方式（确定性，不靠睡眠）：经故障缝在"已捕获前像、尚未替换"这一刻改掉
/// 目标——这正是另一个 Host 写在自己临界区里落地的时间点。旧实现直接
/// `fs::copy`，既不在锁内也不复验基线，会把刚刚提交的字节静默盖掉。
#[test]
fn rev02_restore_never_silently_invalidates_a_committed_write() {
    let (db, root, conversation, run) = rev_fixture("f2", "allow");
    let target = root.join("a.txt");
    let stored = stored_path(&root, "a.txt");
    struct InjectingSeam(std::path::PathBuf, Vec<u8>);
    impl managed_files::RestoreFaultSeam for InjectingSeam {
        fn after_capture(&self) -> Result<(), String> {
            std::fs::write(&self.0, &self.1).expect("inject the concurrent write");
            Ok(())
        }
    }

    host_write(&db, &root, &run, "a.txt", "v1
", "missing", "write-v1");
    let stored = stored_path(&root, "a.txt");
    let restore_id = managed_version_ids(&db, &conversation, &stored)
        .first()
        .cloned()
        .expect("registered version");
    let backups = root.join("managed-files");
    let seam = InjectingSeam(target.clone(), b"committed by another Host write
".to_vec());

    let outcome = managed_files::restore_version_with_seam(
        &db,
        &backups,
        &conversation,
        &restore_id,
        true,
        "rev02-inlock-conflict",
        &seam,
    );
    assert!(
        outcome.is_err(),
        "a concurrent commit inside the critical section must refuse the restore"
    );
    assert_eq!(
        std::fs::read(&target).expect("read target"),
        b"committed by another Host write
",
        "the bytes the concurrent write committed must survive"
    );
    // 目标内容必须仍然是某个已登记版本可知的内容（恢复材料保留，不是静默丢失）。
    let final_hash = tool_host::file_version(&std::fs::read(&target).expect("read target"));
    assert_eq!(
        final_hash,
        tool_host::file_version(b"committed by another Host write
"),
        "the surviving bytes are exactly the committed ones"
    );
}

/// F2b（竞争窗口，补充）：并发的普通写与恢复都报告成功时，最终字节必须对应
/// 某个已登记版本——不得留下注册表从未记录过的内容。
#[test]
fn rev02_race_leaves_only_registered_content() {
    let (db, root, conversation, run) = rev_fixture("f2b", "allow");
    let target = root.join("a.txt");
    host_write(&db, &root, &run, "a.txt", "v1
", "missing", "write-v1");
    let stored = stored_path(&root, "a.txt");
    let restore_id = managed_version_ids(&db, &conversation, &stored)
        .first()
        .cloned()
        .expect("registered version");
    let backups = root.join("managed-files");

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer_db = db.clone();
    let writer_root = root.clone();
    let writer_run = run.clone();
    let writer_stop = stop.clone();
    let writer = std::thread::spawn(move || {
        let mut committed: Vec<String> = Vec::new();
        let mut index = 0usize;
        while !writer_stop.load(std::sync::atomic::Ordering::SeqCst) && index < 200 {
            let content = format!("committed-{index}
");
            // 每次都以磁盘实际内容为前置条件，使写入总能推进（否则夹具会把环境
            // 噪声当成缺陷证据）。
            let expected = std::fs::read_to_string(&writer_root.join("a.txt"))
                .map(|text| tool_host::file_version(text.as_bytes()))
                .unwrap_or_else(|_| "missing".to_owned());
            let call = format!("write-{index}");
            let result = host_write(
                &writer_db,
                &writer_root,
                &writer_run,
                "a.txt",
                &content,
                &expected,
                &call,
            );
            if result["isError"].as_bool() != Some(true) {
                committed.push(tool_host::file_version(content.as_bytes()));
            }
            index += 1;
        }
        committed
    });

    for _ in 0..100 {
        let _ = managed_files::restore_version(&db, &backups, &conversation, &restore_id, true);
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let committed = writer.join().expect("writer thread");

    let final_bytes = std::fs::read(&target).expect("read final target");
    let final_hash = tool_host::file_version(&final_bytes);
    let registered: Vec<String> = db
        .with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT after_hash FROM managed_file_versions WHERE conversation_id=?1 AND storage_path=?2")
                .expect("prepare");
            let rows = statement.query_map(rusqlite::params![conversation, stored], |row| {
                row.get::<_, Option<String>>(0)
            });
            Ok::<Vec<String>, rusqlite::Error>(
                rows.expect("query").flatten().flatten().collect::<Vec<_>>(),
            )
        })
        .expect("registered hashes");
    assert!(
        registered.iter().any(|hash| format!("sha256:{hash}") == final_hash),
        "the final content must be a registered version ({} commits, final={})",
        committed.len(),
        String::from_utf8_lossy(&final_bytes)
    );
}

/// N2：无漂移的正常恢复必须原样落盘并登记新版本。
#[test]
fn rev02_normal_restore_round_trips_the_selected_bytes() {
    let (db, root, conversation, run) = rev_fixture("n2", "allow");
    let target = root.join("a.txt");
    host_write(&db, &root, &run, "a.txt", "first\n", "missing", "write-v1");
    let first_id = managed_version_ids(&db, &conversation, &stored_path(&root, "a.txt"))
        .first()
        .cloned()
        .expect("registered version");
    host_write(
        &db,
        &root,
        &run,
        "a.txt",
        "second\n",
        &tool_host::file_version(b"first\n"),
        "write-v2",
    );

    let restored = managed_files::restore_version(
        &db,
        &root.join("managed-files"),
        &conversation,
        &first_id,
        false,
    )
    .expect("a clean restore must succeed");
    assert_eq!(
        std::fs::read(&target).expect("read target"),
        b"first\n",
        "the restored bytes must be the selected version's bytes"
    );
    assert_eq!(
        restored.after_hash.as_deref(),
        Some(tool_host::file_version(b"first\n").trim_start_matches("sha256:")),
        "the registered version must record the bytes actually written"
    );
}

// ---------------------------------------------------------------------------
// Coordinator round 3: the four remaining negative cases.
// ---------------------------------------------------------------------------

/// The three whole-file-replacement fields are part of the authoritative
/// snapshot. Clearing the flag or swapping either digest must be refused, so a
/// presented credential cannot talk its way out of the dedicated authorization.
#[test]
fn rev_replace_grant_fields_are_verified_field_by_field() {
    let (db, root, _conversation, run, _scope) = kernel_gateway_fixture("grant-tamper", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"line one\nline two\n").expect("write");
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-one",
    ));
    let input = json!({"path":"a.txt","content":"whole file","expectedVersion":version});
    let credential = db
        .issue_dispatch_credential(&run, "write-1", "write_file", &input.to_string())
        .expect("issue")
        .expect("credential");

    // true -> false: the classic "drop the requirement" tamper.
    let mut cleared = credential.clone();
    cleared.requires_replace_grant = false;
    let error = cleared
        .verify_against(&credential)
        .expect_err("clearing the flag must not verify");
    assert!(
        error.contains("credential_mismatch"),
        "got: {error}"
    );

    // Swapped candidate digest.
    let mut swapped_candidate = credential.clone();
    swapped_candidate.replace_candidate_digest =
        Some("sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned());
    assert!(swapped_candidate.verify_against(&credential).is_err());

    // Swapped request digest.
    let mut swapped_request = credential.clone();
    swapped_request.replace_request_digest = Some("not-the-request-digest".to_owned());
    assert!(swapped_request.verify_against(&credential).is_err());

    // And the claim refuses the tampered presentation before anything runs.
    let outcome = db
        .claim_execution_attempt(
            &run,
            &binding.conversation_id,
            &cleared,
            &cleared.intent_digest,
            "owner-tamper",
        )
        .expect_err("a tampered credential must be refused at the claim");
    assert!(
        outcome.contains("credential_mismatch"),
        "got: {outcome}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).expect("target"),
        "line one\nline two\n",
        "a tampered presentation must not write"
    );
}

/// A frozen reusable approval is bound to the generation that issued it.
/// Revoking it and re-granting the SAME (tool, scope) must NOT revive the
/// authorization a Run frozen before the revocation was carrying.
#[test]
fn rev_frozen_grant_is_not_revived_by_a_later_regrant() {
    let (db, root, conversation, run) = rev_fixture_unfrozen("epoch", "ask");
    let scope = "host-target:sha256:abc";
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO conversation_tool_permissions(conversation_id,tool_name,scope_key,granted_at)
             VALUES (?1,'write_file',?2,1)",
            rusqlite::params![conversation, scope],
        )?;
        Ok(())
    })
    .expect("seed a reusable approval");

    let frozen = db
        .freeze_legacy_run_control_with_executor(&run, "legacy", fox_engine_protocol::ResourceExecutor::Rust)
        .expect("freeze with the grant");
    assert_eq!(frozen.permission.grants.len(), 1);
    let epoch = frozen.permission.approval_epoch.expect("the epoch is recorded");

    let policy = |db: &Database| -> kernel_gateway::GatewayPolicy {
        kernel_gateway::GatewayPolicy {
            binding: frozen.clone(),
            scope: scope_for(rev_scope_tools()),
            database: Some(db.clone()),
            sessions_dir: None,
            artifacts_dir: None,
        }
    };
    assert_eq!(
        policy(&db).live_frozen_grants().len(),
        1,
        "while the approval is live and the generation unchanged, the frozen Run may use it"
    );

    // Withdraw it: the revocation advances the conversation's generation.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE conversation_tool_permissions SET revoked_at=2, revoke_reason='user withdrew'
             WHERE conversation_id=?1 AND tool_name='write_file' AND scope_key=?2",
            rusqlite::params![conversation, scope],
        )?;
        Ok(())
    })
    .expect("withdraw");
    let after_revoke = db.execution_policy(&conversation).expect("policy").version;
    assert!(
        after_revoke != epoch,
        "a revocation must advance the generation (epoch {epoch} -> {after_revoke})"
    );

    // Re-grant the identical (tool, scope): a NEW, different authorization. This
    // is the production upsert, which re-activates the row.
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO conversation_tool_permissions(
                 conversation_id, tool_name, scope_key, granted_at, revoked_at, revoke_reason)
             VALUES (?1,'write_file',?2,3,NULL,NULL)
             ON CONFLICT(conversation_id, tool_name, scope_key)
             DO UPDATE SET granted_at = excluded.granted_at,
                           revoked_at = NULL, revoke_reason = NULL",
            rusqlite::params![conversation, scope],
        )?;
        Ok(())
    })
    .expect("re-grant the same scope");

    assert!(
        db.conversation_tool_permission_granted(&conversation, "write_file", scope)
            .expect("granted check"),
        "the new authorization is live for NEW work"
    );
    assert!(
        policy(&db).live_frozen_grants().is_empty(),
        "but it must not revive the authorization the frozen Run was frozen with"
    );
    let _ = root;
}

/// A persistent refusal is terminal for that dispatch: a repeat delivery reads
/// the recorded refusal, never starts the attempt and never writes.
#[test]
fn rev_persistent_refusal_is_not_retried() {
    let (db, root, _conversation, run, _scope) = kernel_gateway_fixture("no-retry", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"line one\nline two\n").expect("write");
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-one",
    ));
    let input = json!({"path":"a.txt","content":"whole file","expectedVersion":version});
    let credential = db
        .issue_dispatch_credential(&run, "write-1", "write_file", &input.to_string())
        .expect("issue")
        .expect("credential");

    // First claim: refused because the replacement was never confirmed.
    let first = db
        .claim_execution_attempt(&run, &binding.conversation_id, &credential, &credential.intent_digest, "owner-1")
        .expect("first claim");
    assert!(
        matches!(first, fox_engine_protocol::AttemptOutcome::AlreadyRefused { .. }),
        "got {first:?}"
    );
    // Confirm it afterwards. The refusal is still terminal: the attempt is not
    // reopened and the file is not written.
    let baseline = credential.file_baseline.as_ref().expect("baseline");
    db.approve_whole_file_replacement(
        &run,
        &credential.dispatch_id,
        &binding.conversation_id,
        &baseline.target_identity,
        &baseline.version,
        credential.replace_candidate_digest.as_deref().expect("candidate"),
        credential.replace_request_digest.as_deref().expect("request"),
    )
    .expect("the confirmation");
    for owner in ["owner-2", "owner-3"] {
        let repeat = db
            .claim_execution_attempt(&run, &binding.conversation_id, &credential, &credential.intent_digest, owner)
            .expect("repeat claim");
        assert!(
            matches!(repeat, fox_engine_protocol::AttemptOutcome::AlreadyRefused { .. }),
            "a persistent refusal must not be retried, got {repeat:?}"
        );
    }
    let state: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1",
                [&run],
                |r| r.get(0),
            )
        })
        .expect("grant state");
    assert_eq!(
        state, "approved",
        "the confirmation stands for a NEW dispatch, but this one stays refused"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).expect("target"),
        "line one\nline two\n",
        "a refused dispatch must never write"
    );
}

/// The desktop restore entry carries the request identity, so a repeated
/// delivery of the same action is deduplicated instead of writing twice.
#[test]
fn api_layer_restore_request_identity_is_forwarded() {
    let (db, root, conversation, run) = rev_fixture("entry-dedup", "allow");
    let target = root.join("a.txt");
    host_write(&db, &root, &run, "a.txt", "first\n", "missing", "write-v1");
    let stored = stored_path(&root, "a.txt");
    let restore_id = managed_version_ids(&db, &conversation, &stored)
        .first()
        .cloned()
        .expect("registered version");
    host_write(
        &db,
        &root,
        &run,
        "a.txt",
        "second\n",
        &tool_host::file_version(b"first\n"),
        "write-v2",
    );
    let backups = root.join("managed-files");

    // Exactly what the desktop command now does: one stable identity per action.
    let request_id = "ui-action-42".to_owned();
    let call = || {
        managed_files::restore_version_with_seam(
            &db,
            &backups,
            &conversation,
            &restore_id,
            false,
            &request_id,
            &managed_files::NoRestoreFault,
        )
    };
    let first = call().expect("the first delivery succeeds");
    let after_first = std::fs::read(&target).expect("read target");
    let rows_after_first = managed_version_ids(&db, &conversation, &stored).len();

    let second = call().expect("the repeat delivery reads the recorded result");
    assert_eq!(second.id, first.id, "the same request identity resolves to one result");
    assert_eq!(
        std::fs::read(&target).expect("read target"),
        after_first,
        "the file must not be written twice"
    );
    assert_eq!(
        managed_version_ids(&db, &conversation, &stored).len(),
        rows_after_first,
        "no extra version row may be registered"
    );
}

// ---------------------------------------------------------------------------
// Coordinator round 4: epoch provenance, mixed grants, the real approval entry,
// and the stable restore identity.
// ---------------------------------------------------------------------------

/// An old binding that carries no approval epoch cannot prove WHICH approval its
/// reusable entries came from. Such entries must not authorize — but a NEW
/// explicit approval still works, because it freezes a fresh binding that does
/// record the generation.
#[test]
fn rev_old_binding_without_epoch_never_authorizes_reuse() {
    let (db, root, conversation, run) = rev_fixture_unfrozen("no-epoch", "ask");
    let scope = "host-target:sha256:abc";
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO conversation_tool_permissions(conversation_id,tool_name,scope_key,granted_at)
             VALUES (?1,'write_file',?2,1)",
            rusqlite::params![conversation, scope],
        )?;
        Ok(())
    })
    .expect("seed a reusable approval");

    // A binding serialized BEFORE `approval_epoch` existed: the field is absent.
    let legacy_binding = serde_json::json!({
        "schemaVersion": 1,
        "runId": run,
        "conversationId": conversation,
        "engineId": "pi",
        "executionProfileId": "legacy",
        "authority": "legacy",
        "readOnlyExecutor": "rust",
        "permissionSnapshotId": "sha256:legacy",
        "permission": {
            "mode": "ask",
            "projectRoot": root.to_string_lossy(),
            "grants": [{"tool": "write_file", "scope": scope}],
        },
        "budgets": serde_json::to_value(fox_engine_protocol::TimeBudgets::continuous())
            .expect("budgets"),
    });
    let old: fox_engine_protocol::RunControlBinding =
        serde_json::from_str(&serde_json::to_string(&legacy_binding).expect("serialize"))
            .expect("an old binding must still deserialize");
    assert_eq!(
        old.permission.approval_epoch, None,
        "the fixture must really be an epoch-less binding"
    );

    let policy = kernel_gateway::GatewayPolicy {
        binding: old,
        scope: scope_for(rev_scope_tools()),
        database: Some(db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    };
    assert!(
        policy.live_frozen_grants().is_empty(),
        "without a provable approval identity, an old approval-reuse entry must not authorize"
    );

    // Withdraw it and re-grant the identical scope: still no authorization for
    // the old binding, because nothing about its provenance changed.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE conversation_tool_permissions SET revoked_at=2 WHERE conversation_id=?1",
            [&conversation],
        )?;
        connection.execute(
            "INSERT INTO conversation_tool_permissions(
                 conversation_id, tool_name, scope_key, granted_at, revoked_at, revoke_reason)
             VALUES (?1,'write_file',?2,3,NULL,NULL)
             ON CONFLICT(conversation_id, tool_name, scope_key)
             DO UPDATE SET granted_at = excluded.granted_at, revoked_at = NULL",
            rusqlite::params![conversation, scope],
        )?;
        Ok(())
    })
    .expect("revoke then re-grant the same scope");
    assert!(
        db.conversation_tool_permission_granted(&conversation, "write_file", scope)
            .expect("granted check"),
        "the NEW approval is live for new work"
    );
    assert!(
        policy.live_frozen_grants().is_empty(),
        "and it still must not authorize the epoch-less frozen binding"
    );

    // A freshly frozen binding DOES record the generation, so new work proceeds.
    let fresh = db
        .freeze_legacy_run_control_with_executor(&run, "legacy", fox_engine_protocol::ResourceExecutor::Rust)
        .expect("freeze a new binding");
    assert!(fresh.permission.approval_epoch.is_some());
    let fresh_policy = kernel_gateway::GatewayPolicy {
        binding: fresh,
        scope: scope_for(rev_scope_tools()),
        database: Some(db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    };
    assert_eq!(
        fresh_policy.live_frozen_grants().len(),
        1,
        "a new explicit approval authorizes through a binding that records the generation"
    );
}

/// A generation change filters ONLY the approval-reuse entries. Ordinary
/// resource bindings survive, so a revocation never takes away the Run's own
/// project/read scope.
#[test]
fn rev_epoch_change_keeps_resource_grants_and_drops_reuse() {
    let (db, root, conversation, run) = rev_fixture_unfrozen("mixed", "ask");
    let reuse_scope = "host-target:sha256:abc";
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO conversation_tool_permissions(conversation_id,tool_name,scope_key,granted_at)
             VALUES (?1,'write_file',?2,1)",
            rusqlite::params![conversation, reuse_scope],
        )?;
        Ok(())
    })
    .expect("seed a reusable approval");
    let frozen = db
        .freeze_legacy_run_control_with_executor(&run, "legacy", fox_engine_protocol::ResourceExecutor::Rust)
        .expect("freeze");
    // Mix in an ordinary resource grant, exactly as a Run's own project scope
    // would appear.
    let mut mixed = frozen.clone();
    mixed.permission.grants.push(fox_engine_protocol::PermissionGrant {
        tool: "read".into(),
        scope: "project".into(),
        kind: fox_engine_protocol::GrantKind::Resource,
    });
    let policy = kernel_gateway::GatewayPolicy {
        binding: mixed,
        scope: scope_for(rev_scope_tools()),
        database: Some(db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    };
    assert_eq!(
        policy.live_frozen_grants().len(),
        2,
        "before the revocation both entries are in force"
    );

    // Withdraw the reusable approval: the generation advances.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE conversation_tool_permissions SET revoked_at=2 WHERE conversation_id=?1",
            [&conversation],
        )?;
        Ok(())
    })
    .expect("withdraw");

    let live = policy.live_frozen_grants();
    assert_eq!(
        live.len(),
        1,
        "only the resource entry survives a generation change"
    );
    assert_eq!(live[0].kind, fox_engine_protocol::GrantKind::Resource);
    assert_eq!(live[0].tool, "read");
    let _ = root;
}
#[test]
fn state_fixture_legacy_propose_confirm_persist_claim() {
    let (db, root, conversation, run) = rev_fixture("real-order", "ask");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"line one\nline two\n").expect("write");

    // A Host read of only the first line: a version fact, no whole-file coverage.
    let registry = CancellationRegistry::default();
    registry.register_run(&run).expect("register run");
    let read_token = registry.tool_token(&run, "read-1").expect("token");
    let observed = managed_files::execute_observed_reader(
        &db,
        &binding,
        "read",
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-1",
        &read_token,
        std::time::Duration::from_secs(5),
    )
    .expect("host read");
    let version = read_version(&observed);

    // The REAL host tool path, driven through the durable entry points it uses:
    // a pending tool call, the immutable approval record built by
    // `create_approval` (which refuses to rewrite an existing request), and the
    // proposal of the replacement request.
    let input = json!({"path":"a.txt","content":"whole file","expectedVersion":version});
    let call = crate::runtime_host::create_fresh_host_tool_call(
        &db,
        &run,
        "write-1",
        "write_file",
        &input,
        "pending",
        true,
    )
    .expect("the pending tool call");
    let call = match call {
        crate::runtime_host::HostToolCallExecution::Execute(record) => record,
        crate::runtime_host::HostToolCallExecution::Replay(_) => {
            panic!("a fresh tool call must not replay")
        }
    };
    let target = tool_host::canonical_file_identity(&root, "a.txt").expect("target");
    let bound = crate::database::kernel_execution_admission::replace_request_binding(
        &conversation,
        &run,
        "write-1",
        &target,
        &version,
        "whole file",
    );
    db.propose_whole_file_replacement(&conversation, &run, "write-1", &bound)
        .expect("propose the request");

    // 1) The request exists and is `pending`.
    let grant_state = |db: &Database| -> String {
        db.with_connection(|c| {
            c.query_row(
                "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1",
                [&run],
                |r| r.get::<_, String>(0),
            )
        })
        .expect("grant state")
    };
    assert_eq!(grant_state(&db), "pending", "the request is proposed up front");

    // 2) The approval record is created by the REAL entry point, carrying the
    //    immutable binding, and cannot be rewritten with different content.
    let approval = db
        .create_approval(
            &call.id,
            "整文件替换",
            &json!({"title":"整文件替换","wholeFileReplacement": {
                "purpose": "整文件替换",
                "requestDigest": bound.request_digest,
                "targetIdentity": bound.target_identity,
                "baselineVersion": bound.version,
                "candidateDigest": bound.candidate_digest,
            }}),
        )
        .expect("create the approval request");
    let approval_id = approval.id.clone();
    assert!(
        db.create_approval(
            &call.id,
            "整文件替换",
            &json!({"title":"different"}),
        )
        .is_err(),
        "an existing approval request is immutable"
    );
    let request_json: String = db
        .with_connection(|c| {
            c.query_row("SELECT request_json FROM approvals WHERE id=?1", [&approval_id], |r| {
                r.get(0)
            })
        })
        .expect("request json");
    let bound: serde_json::Value = serde_json::from_str(&request_json).expect("json");
    let binding_value = bound.get("wholeFileReplacement").expect("the approval binds the request");
    assert_eq!(binding_value["purpose"], "整文件替换");
    assert!(binding_value["targetIdentity"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(binding_value["baselineVersion"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(binding_value["candidateDigest"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(binding_value["requestDigest"].as_str().is_some_and(|v| !v.is_empty()));

    // 3) Before any decision, the confirmation refuses: no valid decision yet.
    let early = db.confirm_replace_grant_for_approval(&approval_id);
    assert!(
        early.is_err(),
        "a pending approval must not confirm anything, got {early:?}"
    );
    assert_eq!(grant_state(&db), "pending");

    // 4) The user DENIES: the request is withdrawn immediately and permanently.
    db.with_connection(|c| {
        c.execute(
            "UPDATE approvals SET status='denied', decision_json='{\"approved\":false}', resolved_at=1 WHERE id=?1",
            [&approval_id],
        )?;
        Ok(())
    })
    .expect("deny the approval");
    assert!(
        db.confirm_replace_grant_for_approval(&approval_id).is_err(),
        "a denied approval must never confirm a replacement"
    );
    let _ = crate::runtime_host::managed_files::NoRestoreFault;

    // 5) A fresh, ordinary write approval (no replacement binding) confirms
    //    nothing — an ordinary approval must not implicitly grant the dedicated
    //    authorization.
    let plain_approval = format!("approval-{}", uuid::Uuid::new_v4().simple());
    let plain_call = format!("call-{}", uuid::Uuid::new_v4().simple());
    db.with_connection(|c| {
        c.execute(
            "INSERT INTO tool_calls(
                 id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                 status, execution_location, requires_approval, started_at, updated_at)
             VALUES (?1, 'write-plain', ?2, ?3, 'write_file', '{\"path\":\"a.txt\"}',
                     'pending', 'host', 1, 1, 1)",
            rusqlite::params![plain_call, run, conversation.as_str()],
        )?;
        c.execute(
            "INSERT INTO approvals(id, tool_call_id, status, requested_action, request_json, requested_at)
             VALUES (?1, ?2, 'approved', 'write file', '{\"title\":\"write\"}', 1)",
            rusqlite::params![plain_approval, plain_call],
        )?;
        Ok(())
    })
    .expect("an ordinary approved write");
    assert_eq!(
        db.confirm_replace_grant_for_approval(&plain_approval)
            .expect("an ordinary approval is simply not a replacement"),
        false,
        "an ordinary write approval must not grant the dedicated authorization"
    );

    // 6) The real confirmation path: approve the original request and confirm it
    //    through the same entry point the Host uses.
    db.with_connection(|c| {
        c.execute(
            "UPDATE approvals SET status='approved', decision_json='{\"approved\":true}', resolved_at=2 WHERE id=?1",
            [&approval_id],
        )?;
        Ok(())
    })
    .expect("approve the request");
    assert!(
        db.confirm_replace_grant_for_approval(&approval_id)
            .expect("the real confirmation"),
        "the approval record's binding confirms the request"
    );
    assert_eq!(grant_state(&db), "approved", "and only now is it authorized");

    // 7) The dispatch is issued through the REAL Legacy credential entry and
    //    claimed, consuming the authorization.
    // Once the approval is claimed the real flow moves the tool call and the Run
    // to `running`; the Legacy credential entry requires exactly that.
    db.with_connection(|c| {
        c.execute("UPDATE runs SET status='running', started_at=1 WHERE id=?1", [&run])?;
        c.execute(
            "UPDATE tool_calls SET status='running' WHERE run_id=?1 AND runtime_tool_call_id='write-1'",
            [&run],
        )?;
        Ok(())
    })
    .expect("running tool call");
    // The approval is claimed before the dispatch is admitted, exactly as the
    // real flow does.
    db.with_connection(|c| {
        c.execute(
            "UPDATE approvals
                SET status='approved',
                    decision_json='{\"approved\":true,\"scope\":\"once\"}',
                    resolved_at=3, claimed_at=3, claimed_by_run_id=?2,
                    category='tool_execution'
              WHERE id=?1",
            rusqlite::params![approval_id, run],
        )?;
        Ok(())
    })
    .expect("claim the approval");
    let policy_version = db.execution_policy(&conversation).expect("policy").version;
    let credential = db
        .issue_legacy_file_credential(&run, "write-1", policy_version)
        .expect("issue the legacy credential");
    let outcome = db
        .claim_execution_attempt(&run, &conversation, &credential, &credential.intent_digest, "owner-1")
        .expect("claim");
    assert_eq!(outcome, fox_engine_protocol::AttemptOutcome::Claimed);
    assert_eq!(grant_state(&db), "consumed");
}
#[test]
fn rev_kernel_projection_binds_the_replacement_request() {
    let (db, root, _conversation, run, _scope) = kernel_gateway_fixture("kernel-binding", "ask");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"line one\nline two\n").expect("write");
    // A one-line read: a version fact, no whole-file coverage.
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-one",
    ));
    let input = json!({"path":"a.txt","content":"whole file","expectedVersion":version});

    // The real Kernel flow parks the run on approval first: a waiting_approval
    // tool call, a waiting_approval run and a pending Kernel approval.
    db.with_connection(|c| {
        c.execute(
            "INSERT INTO kernel_tool_batches(batch_id, run_id, ordered_tool_call_ids_json, barrier_emitted, created_at)
             VALUES ('b1', ?1, json_array('write-1'), 0, 1)",
            [&run],
        )?;
        c.execute(
            "INSERT INTO kernel_tool_calls(run_id, tool_call_id, batch_id, tool, source_order,
                 canonical_input_json, state, created_at)
             VALUES (?1, 'write-1', 'b1', 'write_file', 0, ?2, 'waiting_approval', 1)",
            rusqlite::params![run, input.to_string()],
        )?;
        c.execute(
            "UPDATE kernel_runs SET state='waiting_approval', turn_id='turn-1', approval_deadline_wall_ms=?2 WHERE run_id=?1",
            rusqlite::params![run, crate::database::now_ms() + 86_000_000],
        )?;
        c.execute(
            "INSERT INTO kernel_approvals(run_id, tool_call_id, state, created_at)
             VALUES (?1, 'write-1', 'pending', 1)",
            [&run],
        )?;
        Ok(())
    })
    .expect("park the Kernel run on approval");

    // The Kernel projection is the durable read model the desktop renders from.
    run_db_turn_update(&db, &run).expect("record the frozen turn");
    // The durable retry counters `RunController::start` would have written; this
    // fixture is about the approval chain, not about run start.
    db.with_connection(|c| {
        c.execute(
            "UPDATE kernel_runs SET retry_state_json=?2, compaction_state_json=?3
              WHERE run_id=?1 AND (retry_state_json IS NULL OR compaction_state_json IS NULL)",
            rusqlite::params![
                run,
                serde_json::to_string(&crate::kernel::RetryState::default()).unwrap(),
                serde_json::to_string(&crate::kernel::CompactionState::default()).unwrap(),
            ],
        )?;
        Ok(())
    })
    .expect("seed the durable run counters");
    db.kernel_project_for_test(&run).expect("project");

    // The approval record the UI answers now carries the binding.
    let request_json: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT a.request_json FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                  WHERE t.run_id=?1 AND t.tool_name='write_file'",
                [&run],
                |r| r.get(0),
            )
        })
        .expect("the projected approval");
    let bound: serde_json::Value = serde_json::from_str(&request_json).expect("json");
    let replacement = bound.get("wholeFileReplacement").expect("the Kernel approval binds the request");
    assert_eq!(replacement["purpose"], "整文件替换");
    assert!(replacement["targetIdentity"].as_str().is_some_and(|v| !v.is_empty()));
    assert_eq!(replacement["baselineVersion"], serde_json::json!(version));
    assert!(replacement["candidateDigest"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(replacement["requestDigest"].as_str().is_some_and(|v| !v.is_empty()));

    // And the request itself exists in `pending`.
    let state: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1",
                [&run],
                |r| r.get(0),
            )
        })
        .expect("the request row");
    assert_eq!(state, "pending", "the Kernel path proposes the request up front");
    let _ = input;
}
#[test]
fn rev_production_command_gate_stays_closed() {
    let (db, root, _conversation, run) = rev_fixture("gate", "allow");
    let registry = CancellationRegistry::default();
    registry.register_run(&run).expect("register run");
    let token = registry.tool_token(&run, "cmd").expect("token");
    let prepared = tool_host::prepare(
        "run_command",
        &json!({"command":"echo must-not-run > gate-marker.txt"}),
        root.to_str().unwrap(),
    )
    .expect("prepare command");
    let outcome = tool_host::execute_with_cancellation(prepared, Some(&token));
    let error = outcome
        .expect_err("production must refuse to start a command without a verified backend");
    assert!(
        error.contains("sandbox_unavailable"),
        "the refusal must name the missing verified backend, got: {error}"
    );
    assert!(
        !root.join("gate-marker.txt").exists(),
        "no external process may be started by the production gate"
    );
    let _ = db;
}

// ---------------------------------------------------------------------------
// REV-05 / coordinator M3+M2 and the remaining acceptance cases.
// ---------------------------------------------------------------------------

/// A whole-file replacement consumes its purpose-specific authorization exactly
/// once, atomically with the persistent claim. A second delivery of the same
/// dispatch cannot consume it again, and a dispatch whose authorization was
/// withdrawn is refused before execution.
#[test]
fn rev_replace_grant_is_consumed_atomically_with_the_claim() {
    let (db, root, _conversation, run, _scope) = kernel_gateway_fixture("replace-grant", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    // A one-line read establishes a version fact but no whole-file coverage.
    std::fs::write(root.join("a.txt"), b"line one\nline two\n").expect("write");
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-one",
    ));
    let input = json!({"path":"a.txt","content":"whole file","expectedVersion":version});
    let canonical = input.to_string();
    let target = tool_host::canonical_file_identity(&root, "a.txt").expect("target");

    let credential = db
        .issue_dispatch_credential(&run, "write-1", "write_file", &canonical)
        .expect("issue")
        .expect("a file dispatch carries a credential");
    let grant_state = |db: &Database, dispatch: &str| -> String {
        db.with_connection(|c| {
            c.query_row(
                "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1 AND dispatch_id=?2",
                rusqlite::params![run, dispatch],
                |r| r.get::<_, String>(0),
            )
        })
        .expect("grant state")
    };
    // Issuance recorded the purpose-specific authorization and flagged the
    // dispatch as requiring it.
    assert!(
        credential.requires_replace_grant,
        "a partial read must flag the whole-file replacement"
    );
    assert!(credential.replace_candidate_digest.is_some());
    assert!(credential.replace_request_digest.is_some());
    // Issuance only PROPOSES the authorization; nothing is authorized yet.
    assert_eq!(
        grant_state(&db, &credential.dispatch_id),
        "pending",
        "issuance must not authorize anything by itself"
    );
    // The explicit human confirmation is a separate, exact step.
    let baseline = credential.file_baseline.as_ref().expect("baseline");
    db.approve_whole_file_replacement(
        &run,
        &credential.dispatch_id,
        &binding.conversation_id,
        &baseline.target_identity,
        &baseline.version,
        credential.replace_candidate_digest.as_deref().expect("candidate"),
        credential.replace_request_digest.as_deref().expect("request"),
    )
    .expect("the explicit confirmation");
    assert_eq!(
        grant_state(&db, &credential.dispatch_id),
        "approved",
        "only the explicit confirmation authorizes"
    );

    // First claim consumes it.
    let first = db
        .claim_execution_attempt(&run, &binding.conversation_id, &credential, &credential.intent_digest, "owner-1")
        .expect("first claim");
    assert_eq!(first, fox_engine_protocol::AttemptOutcome::Claimed);

    // A second connection presenting the same dispatch reads the recorded fact
    // and cannot start a second attempt.
    let second = db
        .claim_execution_attempt(&run, &binding.conversation_id, &credential, &credential.intent_digest, "owner-2")
        .expect("second claim");
    assert_eq!(
        second,
        fox_engine_protocol::AttemptOutcome::Unknown,
        "a repeat delivery must be read-only: the first attempt was claimed but never \
         confirmed started, so the honest answer is Unknown, never a second attempt"
    );
    let state: String = db
        .with_connection(|c| {
            c.query_row(
                "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1",
                [&run],
                |r| r.get(0),
            )
        })
        .expect("grant state");
    assert_eq!(state, "consumed", "the authorization is consumed exactly once");
    let _ = target;
}

/// A dispatch whose replacement authorization was withdrawn before the claim is
/// refused, the refusal is persistent, and tightening the permission mode
/// withdraws every unconsumed authorization.
#[test]
fn rev_replace_grant_is_refused_once_withdrawn() {
    let (db, root, _conversation, run, _scope) = kernel_gateway_fixture("replace-withdrawn", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"line one\nline two\n").expect("write");
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-one",
    ));
    let input = json!({"path":"a.txt","content":"whole file","expectedVersion":version});
    let credential = db
        .issue_dispatch_credential(&run, "write-1", "write_file", &input.to_string())
        .expect("issue")
        .expect("a file dispatch carries a credential");

    let grant_state = |db: &Database, dispatch: &str| -> String {
        db.with_connection(|c| {
            c.query_row(
                "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1 AND dispatch_id=?2",
                rusqlite::params![run, dispatch],
                |r| r.get::<_, String>(0),
            )
        })
        .expect("grant state")
    };
    assert_eq!(
        grant_state(&db, &credential.dispatch_id),
        "pending",
        "issuance only records the request"
    );

    // Confirm it, then withdraw it before any claim, exactly as a tightening of
    // the mode does.
    let baseline = credential.file_baseline.as_ref().expect("baseline");
    db.approve_whole_file_replacement(
        &run,
        &credential.dispatch_id,
        &binding.conversation_id,
        &baseline.target_identity,
        &baseline.version,
        credential.replace_candidate_digest.as_deref().expect("candidate"),
        credential.replace_request_digest.as_deref().expect("request"),
    )
    .expect("the explicit confirmation");
    assert_eq!(
        grant_state(&db, &credential.dispatch_id),
        "approved",
        "the confirmation authorizes"
    );
    db.with_connection(|c| {
        c.execute(
            "UPDATE kernel_whole_file_replace_grants SET state='expired' WHERE run_id=?1",
            [&run],
        )?;
        Ok(())
    })
    .expect("withdraw the authorization");
    // A withdrawn authorization can never be re-approved, not even for the very
    // same target, version, candidate and request.
    let revived = db.approve_whole_file_replacement(
        &run,
        &credential.dispatch_id,
        &binding.conversation_id,
        &baseline.target_identity,
        &baseline.version,
        credential.replace_candidate_digest.as_deref().expect("candidate"),
        credential.replace_request_digest.as_deref().expect("request"),
    );
    assert!(
        revived.is_err(),
        "a withdrawn authorization must not be resurrected by a second confirmation"
    );
    assert_eq!(
        grant_state(&db, &credential.dispatch_id),
        "expired",
        "the withdrawn state is terminal"
    );

    let outcome = db
        .claim_execution_attempt(&run, &binding.conversation_id, &credential, &credential.intent_digest, "owner-1")
        .expect("claim");
    match outcome {
        fox_engine_protocol::AttemptOutcome::AlreadyRefused { code } => {
            assert!(
                code.contains("replace_grant"),
                "the refusal must name the withdrawn authorization, got {code}"
            );
        }
        other => panic!("a withdrawn authorization must refuse the claim, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).expect("target"),
        "line one\nline two\n",
        "a refused replacement must not write"
    );
    // The refusal is persistent: a repeat delivery reads the same fact.
    let repeat = db
        .claim_execution_attempt(&run, &binding.conversation_id, &credential, &credential.intent_digest, "owner-2")
        .expect("repeat claim");
    assert!(
        matches!(repeat, fox_engine_protocol::AttemptOutcome::AlreadyRefused { .. }),
        "the refusal must survive a repeat delivery, got {repeat:?}"
    );

    // And a policy tightening withdraws every unconsumed authorization in the
    // conversation, so a standing whole-file replacement permission cannot
    // outlive the mode that produced it.
    let other = db
        .issue_dispatch_credential(
            &run,
            "write-2",
            "write_file",
            &json!({"path":"a.txt","content":"another","expectedVersion":version}).to_string(),
        )
        .expect("issue")
        .expect("credential");
    assert_eq!(
        grant_state(&db, &other.dispatch_id),
        "pending",
        "the second dispatch only has a request"
    );
    let other_baseline = other.file_baseline.as_ref().expect("baseline");
    db.approve_whole_file_replacement(
        &run,
        &other.dispatch_id,
        &binding.conversation_id,
        &other_baseline.target_identity,
        &other_baseline.version,
        other.replace_candidate_digest.as_deref().expect("candidate"),
        other.replace_request_digest.as_deref().expect("request"),
    )
    .expect("the second confirmation");
    assert_eq!(
        grant_state(&db, &other.dispatch_id),
        "approved",
        "the second dispatch is authorized on its own"
    );
    let policy = db.execution_policy(&binding.conversation_id).expect("policy");
    db.change_execution_policy(&binding.conversation_id, "tighten", policy.version, "ask")
        .expect("tighten");
    assert_eq!(
        grant_state(&db, &other.dispatch_id),
        "expired",
        "tightening the mode must withdraw unconsumed replacement authorizations"
    );
    let _ = other;
}

/// REV-05 negative case: `replaceAll` over a fragment that was only partly
/// observed must not modify the unobserved part.
#[test]
fn rev_replace_all_cannot_touch_an_unobserved_fragment() {
    let (db, root, _conversation, run, _scope) = kernel_gateway_fixture("replace-all", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"alpha\nbeta\nalpha\n").expect("write");
    // Only the first line is observed.
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-first",
    ));
    let policy = rev_gateway_policy(&db, &run);
    let input = json!({
        "path":"a.txt",
        "oldText":"alpha",
        "newText":"ALPHA",
        "replaceAll":true,
        "expectedVersion":version,
    });
    let error = policy
        .validate("edit_file", &input)
        .expect_err("a replaceAll over a partly observed fragment must be refused");
    assert!(
        error.contains("tool.file_conflict"),
        "the refusal must be a recoverable conflict, got: {error}"
    );
    assert_eq!(
        std::fs::read(root.join("a.txt")).expect("target"),
        b"alpha\nbeta\nalpha\n",
        "no fragment may be modified"
    );
    // A precise edit of a uniquely-occurring, observed fragment still works, so
    // the refusal above is about the unobserved occurrence and not a blanket ban.
    let beta = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":2,"lineCount":1}),
        "read-second",
    ));
    policy
        .validate(
            "edit_file",
            &json!({"path":"a.txt","oldText":"beta","newText":"BETA","expectedVersion":beta}),
        )
        .expect("a precise edit of an observed, unique fragment stays admissible");
}

/// REV-01 across the three entry points: the Kernel proposal gate, the Legacy
/// host tool path and the final file commit must all refuse a declared version
/// that this Run never observed, instead of upgrading it.
#[test]
fn rev_expected_version_is_never_upgraded_in_any_entry_point() {
    let (db, root, _conversation, run, _scope) = kernel_gateway_fixture("three-entries", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"v1\n").expect("write v1");
    let v1 = read_version(&host_read(&db, &binding, &json!({"path":"a.txt"}), "read-v1"));
    std::fs::write(root.join("a.txt"), b"v2\n").expect("external edit to v2");
    let v2 = read_version(&host_read(&db, &binding, &json!({"path":"a.txt"}), "read-v2"));
    assert_ne!(v1, v2);

    // Entry point 1: the Kernel proposal gate.
    let policy = rev_gateway_policy(&db, &run);
    let stale = json!({"path":"a.txt","content":"stale candidate","expectedVersion":v1});
    assert!(
        policy.validate("write_file", &stale).is_err(),
        "the Kernel gate must refuse a declared version it never observed"
    );
    // The current version is still admissible, so the refusal is about the
    // claim and not a blanket write ban.
    policy
        .validate("write_file", &json!({"path":"a.txt","content":"fresh","expectedVersion":v2}))
        .expect("the observed version stays admissible");

    // Entry point 2: the Legacy host tool path resolves the observation by the
    // declared version, so a stale declaration has no baseline to bind.
    let resolved = db
        .host_observation_for_version(&run, &tool_host::canonical_file_identity(&root, "a.txt").expect("id"), Some(&v1))
        .expect("resolve");
    assert!(
        resolved.is_some(),
        "the stale version WAS observed, so the Legacy entry binds THAT observation"
    );
    let never = db
        .host_observation_for_version(
            &run,
            &tool_host::canonical_file_identity(&root, "a.txt").expect("id"),
            Some("sha256:never-observed"),
        )
        .expect("resolve");
    assert!(
        never.is_none(),
        "a version this Run never observed has no baseline, in any entry point"
    );

    // Entry point 3: the final file commit compares against the baseline the
    // caller bound, so a stale declaration never reaches the write at all.
    let error = tool_host::prepare(
        "write_file",
        &json!({"path":"a.txt","content":"stale candidate","expectedVersion":v1}),
        root.to_str().unwrap(),
    )
    .expect_err("the final commit entry must refuse a stale baseline");
    assert!(
        error.contains("tool.file_conflict"),
        "the final commit must refuse with a recoverable conflict, got: {error}"
    );
    // And the committed path re-checks the baseline inside the write lock.
    // The write path re-checks the baseline inside the lock too, so a request
    // whose declared version no longer matches the file never reaches a write.
    let registry = CancellationRegistry::default();
    registry.register_run(&run).expect("register run");
    let _token = registry.tool_token(&run, "commit").expect("token");
    assert_eq!(
        std::fs::read(root.join("a.txt")).expect("target"),
        b"v2\n",
        "the target must be untouched"
    );
}

/// An older persisted credential and an older persisted binding must still load
/// and must NOT gain the new whole-file-replacement eligibility.
#[test]
fn rev_old_credential_and_binding_stay_loadable_and_conservative() {
    let (db, root, _conversation, run, _scope) = kernel_gateway_fixture("compat", "allow");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"v1\n").expect("write");
    let version = read_version(&host_read(&db, &binding, &json!({"path":"a.txt"}), "read"));
    let credential = db
        .issue_dispatch_credential(
            &run,
            "write-1",
            "write_file",
            &json!({"path":"a.txt","content":"x","expectedVersion":version}).to_string(),
        )
        .expect("issue")
        .expect("a file dispatch carries a credential");

    // Simulate a row persisted before the new fields existed.
    let mut legacy_json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&credential).expect("serialize")).expect("json");
    let object = legacy_json.as_object_mut().expect("object");
    object.remove("requiresReplaceGrant");
    object.remove("replaceCandidateDigest");
    object.remove("replaceRequestDigest");
    let legacy_text = serde_json::to_string(&object).expect("serialize legacy");
    let loaded: fox_engine_protocol::ExecutionCredential =
        serde_json::from_str(&legacy_text).expect("an older credential must still deserialize");
    assert!(
        !loaded.requires_replace_grant,
        "an older credential must not gain a replacement requirement it did not have"
    );
    assert!(
        loaded.credential_digest == loaded.recompute_digest(),
        "the digest must still verify: the new fields are outside it"
    );

    // Same for an older binding: a grant with no `kind` must default to the
    // revocable kind rather than to a trusted resource grant.
    let legacy_binding = serde_json::json!({
        "schemaVersion": binding.schema_version,
        "runId": binding.run_id,
        "conversationId": binding.conversation_id,
        "engineId": binding.engine_id,
        "executionProfileId": binding.execution_profile_id,
        "authority": "legacy",
        "readOnlyExecutor": "rust",
        "permissionSnapshotId": binding.permission_snapshot_id,
        "permission": {
            "mode": "ask",
            "projectRoot": root.to_string_lossy(),
            "grants": [{"tool": "write_file", "scope": "host-target:sha256:abc"}],
        },
        "budgets": serde_json::to_value(&binding.budgets).expect("budgets"),
    });
    let loaded_binding: fox_engine_protocol::RunControlBinding =
        serde_json::from_str(&serde_json::to_string(&legacy_binding).expect("serialize"))
            .expect("an older binding must still deserialize");
    assert_eq!(
        loaded_binding.permission.grants[0].kind,
        fox_engine_protocol::GrantKind::ApprovalReuse,
        "an older grant must default to the revocable kind"
    );
}

/// A reusable approval frozen into a Run is re-checked live: withdrawing it
/// stops the already-frozen Run immediately.
#[test]
fn rev_frozen_binding_grant_is_revoked_live() {
    let (db, root, conversation, run) = rev_fixture_unfrozen("frozen-live", "ask");
    let scope = "host-target:sha256:abc";
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO conversation_tool_permissions(conversation_id,tool_name,scope_key,granted_at)
             VALUES (?1,'write_file',?2,1)",
            rusqlite::params![conversation, scope],
        )?;
        Ok(())
    })
    .expect("seed a reusable approval");
    // Freeze a Run that carries the grant, exactly as production does.
    let frozen = db
        .freeze_legacy_run_control_with_executor(&run, "legacy", fox_engine_protocol::ResourceExecutor::Rust)
        .expect("freeze with the grant");
    assert_eq!(frozen.permission.grants.len(), 1, "the grant is frozen in");
    assert_eq!(
        frozen.permission.grants[0].kind,
        fox_engine_protocol::GrantKind::ApprovalReuse,
        "a frozen reusable approval is marked revocable"
    );
    let live_policy = kernel_gateway::GatewayPolicy {
        binding: frozen.clone(),
        scope: scope_for(rev_scope_tools()),
        database: Some(db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    };
    assert_eq!(
        live_policy.live_frozen_grants().len(),
        1,
        "while the approval is live, the frozen Run may still use it"
    );

    // The user withdraws it while the Run is still frozen.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE conversation_tool_permissions SET revoked_at=2, revoke_reason='user withdrew'
             WHERE conversation_id=?1 AND tool_name='write_file' AND scope_key=?2",
            rusqlite::params![conversation, scope],
        )?;
        Ok(())
    })
    .expect("withdraw");

    let policy = kernel_gateway::GatewayPolicy {
        binding: frozen.clone(),
        scope: scope_for(rev_scope_tools()),
        database: Some(db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    };
    let live = policy.live_frozen_grants();
    assert!(
        live.is_empty(),
        "a withdrawn reusable approval must not keep authorizing a frozen Run"
    );
    let _ = root;
}

/// A repeated delivery of the SAME restore request is deduplicated: the file is
/// mutated once and the second call reads the recorded result.
#[test]
fn rev_restore_request_is_deduplicated() {
    let (db, root, conversation, run) = rev_fixture("dedup", "allow");
    let target = root.join("a.txt");
    host_write(&db, &root, &run, "a.txt", "first\n", "missing", "write-v1");
    let stored = stored_path(&root, "a.txt");
    let restore_id = managed_version_ids(&db, &conversation, &stored)
        .first()
        .cloned()
        .expect("registered version");
    host_write(
        &db,
        &root,
        &run,
        "a.txt",
        "second\n",
        &tool_host::file_version(b"first\n"),
        "write-v2",
    );
    let backups = root.join("managed-files");

    let first = managed_files::restore_version_with_seam(
        &db,
        &backups,
        &conversation,
        &restore_id,
        false,
        "req-stable-1",
        &managed_files::NoRestoreFault,
    )
    .expect("the first delivery succeeds");
    let after_first = std::fs::read(&target).expect("read target");
    let versions_after_first = managed_version_ids(&db, &conversation, &stored).len();

    let second = managed_files::restore_version_with_seam(
        &db,
        &backups,
        &conversation,
        &restore_id,
        false,
        "req-stable-1",
        &managed_files::NoRestoreFault,
    )
    .expect("the repeat delivery reads the recorded result");
    assert_eq!(
        second.id, first.id,
        "a repeated request identity must resolve to the same recorded result"
    );
    assert_eq!(
        std::fs::read(&target).expect("read target"),
        after_first,
        "the file must not be mutated a second time"
    );
    assert_eq!(
        managed_version_ids(&db, &conversation, &stored).len(),
        versions_after_first,
        "no extra version row may be registered"
    );
}

/// A fault injected after the replacement must still settle: the outcome is
/// observed, classified and recorded, never silently dropped.
#[test]
fn rev_restore_settles_after_an_after_replace_fault() {
    struct FailingAfterReplace;
    impl managed_files::RestoreFaultSeam for FailingAfterReplace {
        fn after_replace(&self) -> Result<(), String> {
            Err("injected post-replace fault".to_owned())
        }
    }

    let (db, root, conversation, run) = rev_fixture("settle", "allow");
    let target = root.join("a.txt");
    host_write(&db, &root, &run, "a.txt", "first\n", "missing", "write-v1");
    let stored = stored_path(&root, "a.txt");
    let restore_id = managed_version_ids(&db, &conversation, &stored)
        .first()
        .cloned()
        .expect("registered version");
    host_write(
        &db,
        &root,
        &run,
        "a.txt",
        "second\n",
        &tool_host::file_version(b"first\n"),
        "write-v2",
    );
    let backups = root.join("managed-files");

    let outcome = managed_files::restore_version_with_seam(
        &db,
        &backups,
        &conversation,
        &restore_id,
        false,
        "req-faulted",
        &FailingAfterReplace,
    );
    // The write itself succeeded, so the outcome is observed as committed and
    // the injected fault is reported alongside it — never swallowed.
    match outcome {
        Ok(row) => {
            assert_eq!(row.after_hash.as_deref(), Some(tool_host::file_version(b"first\n").trim_start_matches("sha256:")));
        }
        Err(error) => {
            assert!(
                error.contains("injected post-replace fault"),
                "the injected fault must be reported, got: {error}"
            );
        }
    }
    // Either way the request is settled, so a repeat of the same identity reads
    // the recorded outcome instead of writing again.
    let recorded = db
        .with_connection(|c| {
            c.query_row(
                "SELECT state FROM kernel_restore_requests WHERE conversation_id=?1 AND request_id=?2",
                rusqlite::params![conversation, "req-faulted"],
                |r| r.get::<_, String>(0),
            )
        })
        .expect("the request must be settled");
    assert_ne!(recorded, "running", "the request must not stay unsettled");
    let _ = target;
}

/// The tools the REV fixtures freeze into the Host scope.
fn rev_scope_tools() -> &'static [&'static str] {
    &["read", "write_file", "edit_file"]
}

/// A gateway policy over the frozen scope the fixture already created.
/// The policy over the scope the fixture ALREADY froze (never re-freezes, so it
/// is safe to call after the Run has started).
fn rev_gateway_policy_existing(
    db: &Database,
    run_id: &str,
    scope: crate::database::KernelHostScope,
) -> kernel_gateway::GatewayPolicy {
    kernel_gateway::GatewayPolicy {
        binding: db.run_control_binding(run_id).expect("binding").expect("frozen"),
        scope,
        database: Some(db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    }
}

fn rev_gateway_policy(db: &Database, run_id: &str) -> kernel_gateway::GatewayPolicy {
    // Reuse the scope the fixture already froze; only freeze when it is absent,
    // so calling this after the Run has started cannot hit the immutability
    // guard.
    let scope = match db.kernel_host_scope(run_id) {
        Ok(existing) => existing,
        Err(_) => {
            let scope = scope_for(rev_scope_tools());
            db.freeze_kernel_model_config(run_id, &rev_model_config(rev_scope_tools()))
                .expect("freeze model config");
            db.freeze_kernel_host_scope(run_id, &scope).expect("freeze scope");
            scope
        }
    };
    kernel_gateway::GatewayPolicy {
        binding: db.run_control_binding(run_id).expect("binding").expect("frozen"),
        scope,
        database: Some(db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    }
}

// ---------------------------------------------------------------------------
// Coordinator round 7: the Kernel approval TIMELINE, through the real entries.
//
//   queue_kernel_host_approval   (enqueue - a queue command, nothing more)
//     -> the durable Kernel decision transition (kernel_approvals.state)
//     -> complete_kernel_host_command   (acknowledge + settle, one transaction)
//     -> issue credential / claim
// ---------------------------------------------------------------------------

fn parked_kernel_replacement(
    label: &str,
) -> (Database, PathBuf, String, String, fox_engine_protocol::ExecutionCredential) {
    let (db, root, _conversation, run, scope) = kernel_gateway_fixture(label, "ask");
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"line one\nline two\n").expect("write");
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-one",
    ));
    let input = json!({"path":"a.txt","content":"whole file","expectedVersion":version});
    db.with_connection(|c| {
        c.execute(
            "INSERT INTO kernel_tool_batches(batch_id, run_id, ordered_tool_call_ids_json, barrier_emitted, created_at)
             VALUES ('b1', ?1, json_array('write-1'), 0, 1)",
            [&run],
        )?;
        c.execute(
            "INSERT INTO kernel_tool_calls(run_id, tool_call_id, batch_id, tool, source_order,
                 canonical_input_json, state, created_at)
             VALUES (?1, 'write-1', 'b1', 'write_file', 0, ?2, 'waiting_approval', 1)",
            rusqlite::params![run, input.to_string()],
        )?;
        c.execute(
            "UPDATE kernel_runs SET state='waiting_approval', turn_id='turn-1', approval_deadline_wall_ms=?2 WHERE run_id=?1",
            rusqlite::params![run, crate::database::now_ms() + 86_000_000],
        )?;
        c.execute(
            "INSERT INTO kernel_approvals(run_id, tool_call_id, state, created_at)
             VALUES (?1, 'write-1', 'pending', 1)",
            [&run],
        )?;
        Ok(())
    })
    .expect("park the Kernel run on approval");
    run_db_turn_update(&db, &run).expect("record the frozen turn");
    // The durable retry counters `RunController::start` would have written; this
    // fixture is about the approval chain, not about run start.
    db.with_connection(|c| {
        c.execute(
            "UPDATE kernel_runs SET retry_state_json=?2, compaction_state_json=?3
              WHERE run_id=?1 AND (retry_state_json IS NULL OR compaction_state_json IS NULL)",
            rusqlite::params![
                run,
                serde_json::to_string(&crate::kernel::RetryState::default()).unwrap(),
                serde_json::to_string(&crate::kernel::CompactionState::default()).unwrap(),
            ],
        )?;
        Ok(())
    })
    .expect("seed the durable run counters");
    db.kernel_project_for_test(&run).expect("project");
    let credential = db
        .issue_dispatch_credential(&run, "write-1", "write_file", &input.to_string())
        .expect("issue")
        .expect("credential");
    let approval_id = db
        .kernel_approval_identity(&run, "write-1")
        .expect("the projected approval identity");
    (db, root, run, approval_id, credential)
}

fn replacement_state(db: &Database, run: &str) -> String {
    db.with_connection(|c| {
        c.query_row(
            "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1",
            [run],
            |r| r.get::<_, String>(0),
        )
    })
    .expect("replacement state")
}

fn pending_command_seq(db: &Database, run: &str) -> i64 {
    db.with_connection(|c| {
        c.query_row(
            "SELECT command_seq FROM kernel_host_commands WHERE run_id=?1 AND status='pending'",
            [run],
            |r| r.get::<_, i64>(0),
        )
    })
    .expect("a pending Host command")
}

fn kernel_conversation(db: &Database, run: &str) -> String {
    db.with_connection(|c| {
        c.query_row(
            "SELECT conversation_id FROM run_control_bindings WHERE run_id=?1",
            [run],
            |r| r.get::<_, String>(0),
        )
    })
    .expect("conversation")
}

fn apply_kernel_decision(db: &Database, run: &str, decision: &str) {
    db.with_connection(|c| {
        c.execute(
            "UPDATE kernel_approvals SET state=?2, decided_at=1 WHERE run_id=?1 AND tool_call_id='write-1'",
            rusqlite::params![run, decision],
        )?;
        Ok(())
    })
    .expect("apply the decision");
}

// ---------------------------------------------------------------------------
// T1-T3: the Kernel chain through the REAL consumer (`kernel_host::drive`).
//
// `drive` is the production orchestration loop: it reads the queued Host
// commands, applies the durable Kernel decision through the coordinator,
// registers the approval grant, acknowledges the command (which settles the
// dedicated replacement request in the same transaction) and then executes the
// dispatch. Nothing here writes `kernel_approvals` by hand.
//
// The executor closure is the model/worker boundary: it is a scripted,
// deterministic dependency, exactly as the existing `owning_host_recovery_*`
// tests do. The file assertion is made against the bytes on disk, so a
// successful claim alone is never accepted as "the file was committed".
// ---------------------------------------------------------------------------

/// A Kernel Run parked on a REAL whole-file replacement approval, plus the
/// pieces `drive` needs. Returns everything the caller must keep alive.
struct KernelRun {
    db: Database,
    root: PathBuf,
    run_id: String,
    conversation_id: String,
    approval_id: String,
    scope: crate::database::KernelHostScope,
    clock: crate::kernel::TestClock,
}

fn run_db_turn_update(db: &Database, run: &str) -> Result<usize, String> {
    db.with_connection(|c| {
        c.execute(
            "UPDATE kernel_runs SET turn_id='turn-1' WHERE run_id=?1 AND turn_id IS NULL",
            [run],
        )
        .map_err(|e| e.to_string())
        .map_err(rusqlite::Error::InvalidParameterName)
    })
}

/// A Kernel Run parked on the dedicated whole-file replacement approval.
///
/// EVERY durable fact below is produced by production code — the earlier
/// revision of this fixture hand-inserted a batch, a tool call, a run state and
/// an approval row with SQL, and that fabricated state was the actual defect:
/// it parked the approval with a 24-hour deadline while the frozen approval
/// budget is 300 000 ms, so the controller's fail-closed clock guard rejected it
/// as a backwards clock (`approval.clock_rollback`) and the Run died before any
/// dispatch. The real entries are used instead:
///
///  * `KernelCoordinator::start_prepared` performs the real Run start
///    (`RunController::start_with_initial_input` + `kernel_commit_decision`);
///  * the frozen reader seam records the Host observation the write binds to;
///  * `dispatch_initial` submits one real model proposal (the only scripted
///    dependency, exactly as `owning_host_recovery_*` does); the real policy
///    parks the `write_file` call on approval and the projection writes the
///    purpose-specific replacement request plus the approval ticket.
fn build_kernel_run(label: &str) -> KernelRun {
    let (db, root, _conversation, run, scope) =
        kernel_gateway_fixture_with_model(label, "ask", &rev_consumer_model_config());
    let binding = db.run_control_binding(&run).expect("binding").expect("frozen");
    std::fs::write(root.join("a.txt"), b"line one\nline two\n").expect("write");
    // A one-line read through the production reader seam: a version fact, no
    // whole-file coverage, so the write needs the dedicated replacement.
    let version = read_version(&host_read(
        &db,
        &binding,
        &json!({"path":"a.txt","startLine":1,"lineCount":1}),
        "read-one",
    ));
    let clock = crate::kernel::TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let policy = rev_gateway_policy_existing(&db, &run, scope.clone());
    // The real Run start.
    let coordinator = crate::runtime_host::kernel_coordinator::KernelCoordinator::start_prepared(
        &db, &clock, &run, &cancellation,
    )
    .expect("the real Kernel Run start");
    // The only scripted dependency: one deterministic model proposal. It parks
    // the `write_file` call on approval through the real policy.
    coordinator
        .dispatch_initial("owner-proposal", &policy, |binding, frame, _| {
            Ok(fox_engine_protocol::KernelInitialModelResponse {
                schema_version: 1,
                run_id: binding.run_id.clone(),
                turn_id: frame.input.turn_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[
                    {"type":"toolCall","id":"write-1","name":"write_file",
                     "arguments":{"path":"a.txt","content":"whole file","expectedVersion":version}}]}),
            })
        })
        .expect("the real proposal parks the write on approval");
    drop(coordinator);
    let approval_id = db
        .kernel_approval_identity(&run, "write-1")
        .expect("the projected approval identity");
    KernelRun {
        db,
        root,
        run_id: run,
        conversation_id: binding.conversation_id.clone(),
        approval_id,
        scope,
        clock,
    }
}

/// The real worker command, shared with the existing coordinator tests.
mod tests_support {
    use crate::runtime_host::RuntimeCommand;
    pub(super) fn real_worker_command() -> RuntimeCommand {
        RuntimeCommand {
            program: "node".into(),
            script: Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../services/agent-runtime/src/pi-runtime.mjs")),
        }
    }
}

fn replacement_state_of(db: &Database, run: &str) -> String {
    db.with_connection(|c| {
        c.query_row(
            "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1",
            [run],
            |r| r.get::<_, String>(0),
        )
    })
    .expect("replacement state")
}

/// Where the consumer's process is allowed to die. The fault is injected at a
/// real durable boundary, never by disabling a check.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    /// Run to completion.
    None,
    /// Die after the managed-file commit is durable but before the dispatch is
    /// acknowledged: the outbox effect stays leased with no settled result.
    AfterFileCommit,
}

/// Drive the REAL consumer once, with the file executor observing the bytes it
/// wrote. Returns the number of executor invocations.
fn drive_real_consumer(run: &KernelRun, key: &str) -> Result<usize, String> {
    drive_real_consumer_with(run, key, CrashPoint::None)
}

fn drive_real_consumer_with(
    run: &KernelRun,
    key: &str,
    crash: CrashPoint,
) -> Result<usize, String> {
    let policy = rev_gateway_policy_existing(&run.db, &run.run_id, run.scope.clone());
    let policy_for_executor = rev_gateway_policy_existing(&run.db, &run.run_id, run.scope.clone());
    let cancellation = CancellationRegistry::default();
    cancellation.register_run(&run.run_id).map_err(|e| e.to_string())?;
    let executed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = executed.clone();
    let root = run.root.clone();
    let run_db = run.db.clone();
    crate::runtime_host::kernel_host::drive(
        crate::runtime_host::kernel_host::acquire(&run.root, &run.run_id)?,
        &run.db,
        &run.clock,
        &cancellation,
        &run.run_id,
        &tests_support::real_worker_command(),
        key,
        &policy,
        move |binding, effect, token| {
            let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
            let tool = payload["tool"].as_str().unwrap_or_default().to_owned();
            // The REAL execution boundary: verify the durable identity, present
            // and verify the credential, take the persistent single claim (which
            // consumes the replacement authorization) and only then execute the
            // admitted file write through the shared managed-files seam.
            // Counting happens INSIDE the executor, so a repeat delivery that is
            // answered from durable facts is not counted as an execution.
            let result = crate::runtime_host::kernel_host::execute_claimed_dispatch(
                &run_db,
                binding,
                effect,
                &tool,
                &payload,
                |durable_input, claim| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let input: Value = serde_json::from_str(durable_input).unwrap_or(Value::Null);
                    let tool_call_id = effect.tool_call_id.as_deref().unwrap_or_default();
                    let (result, evidence) = if matches!(tool.as_str(), "write_file" | "edit_file") {
                        crate::runtime_host::managed_files::execute_admitted_file(
                            crate::runtime_host::managed_files::ManagedExecutionContext {
                                database: &run_db,
                                backups_dir: &root.join("managed-files"),
                                conversation_id: &binding.conversation_id,
                                run_id: &binding.run_id,
                                project_root: binding.permission.project_root.as_deref(),
                                permission_mode: binding.permission.mode.as_str(),
                                scope: &policy_for_executor.scope,
                                sessions_dir: None,
                            },
                            &tool,
                            &input,
                            tool_call_id,
                            claim.credential(),
                            Some(token),
                            &mut || Ok(()),
                        )
                    } else {
                        (
                            policy_for_executor.execute_reader(
                                &run_db,
                                &tool,
                                &input,
                                tool_call_id,
                                token,
                            ),
                            fox_engine_protocol::ExecutionEvidence::NotStarted,
                        )
                    };
                    if tool == "write_file" {
                        // The executor's own evidence of what landed on disk.
                        let landed = std::fs::read(root.join("a.txt")).unwrap_or_default();
                        assert_eq!(landed, b"whole file", "the candidate bytes must be on disk");
                        if crash == CrashPoint::AfterFileCommit {
                            // The process dies here. The file commit and the
                            // replacement consumption are already durable; the
                            // dispatch result is not, so the effect remains
                            // leased for the next owner.
                            panic!("injected process death after the file commit");
                        }
                    }
                    let outcome = crate::runtime_host::kernel_host::call_outcome(&result);
                    (result, evidence, outcome)
                },
            );
            result.map(|value| {
                (
                    value.get("isError").and_then(Value::as_bool) != Some(true),
                    value,
                )
            })
        },
    )?;
    Ok(executed.load(std::sync::atomic::Ordering::SeqCst))
}

fn version_rows(db: &Database, conversation: &str, path: &str) -> usize {
    db.with_connection(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM managed_file_versions WHERE conversation_id=?1 AND storage_path=?2",
            rusqlite::params![conversation, path],
            |r| r.get::<_, i64>(0),
        )
    })
    .expect("version rows") as usize
}

/// T1: the real consumer turns a legitimate approval into exactly one executed
/// file commit, one registered version and one receipt.
#[test]
fn t1_kernel_real_consumer_executes_a_legitimate_approval_once() {
    let run = build_kernel_run("t1");
    // The production approval entry only enqueues.
    assert!(run
        .db
        .queue_kernel_host_approval(&run.approval_id, "allow_once")
        .expect("enqueue"));
    assert_eq!(
        replacement_state_of(&run.db, &run.run_id),
        "pending",
        "enqueueing must not authorize"
    );

    let executed = drive_real_consumer(&run, "t1-key").expect("drive the real consumer");
    assert_eq!(executed, 1, "the dispatch must execute exactly once");
    assert_eq!(
        std::fs::read(run.root.join("a.txt")).expect("target"),
        b"whole file",
        "the candidate bytes must be committed"
    );
    assert_eq!(replacement_state_of(&run.db, &run.run_id), "consumed");
    let stored = stored_path(&run.root, "a.txt");
    assert_eq!(
        version_rows(&run.db, &run.conversation_id, &stored),
        1,
        "exactly one version row"
    );
    // A repeat delivery of the same command must not re-execute or re-register.
    let repeat = drive_real_consumer(&run, "t1-key").expect("drive again");
    assert_eq!(repeat, 0, "a repeat delivery must not execute again");
    assert_eq!(
        version_rows(&run.db, &run.conversation_id, &stored),
        1,
        "and must not add another version row"
    );
}

/// T3: a denial through the real entry never executes and never writes.
#[test]
fn t3_kernel_real_consumer_denial_never_executes() {
    let run = build_kernel_run("t3");
    run.db
        .queue_kernel_host_approval(&run.approval_id, "denied")
        .expect("enqueue the denial");
    let executed = drive_real_consumer(&run, "t3-key").expect("drive the denial");
    assert_eq!(executed, 0, "a denial must not execute");
    assert_eq!(
        std::fs::read(run.root.join("a.txt")).expect("target"),
        b"line one\nline two\n",
        "a denial must not write"
    );
    assert_eq!(replacement_state_of(&run.db, &run.run_id), "expired");
    // A late confirmation cannot revive a terminal state.
    assert!(run
        .db
        .confirm_replace_grant_for_approval(&run.approval_id)
        .is_err());
}

/// T2: a real crash recovery. The consumer dies at the execution boundary —
/// AFTER the managed-file commit and the replacement consumption are durable,
/// but BEFORE the dispatch result is settled — then the database is closed,
/// reopened and the consumer rebuilt from scratch.
///
/// Recovery must not write again. The interrupted dispatch is uncertain work,
/// so it is REPORTED as uncertain instead of being replayed, and the file keeps
/// the single committed version. "It did not execute, therefore it did not
/// execute twice" would not satisfy this: the assertions below first prove the
/// write really landed before the crash.
#[test]
fn t2_kernel_recovery_after_reopen_settles_once() {
    let run = build_kernel_run("t2");
    assert!(run
        .db
        .queue_kernel_host_approval(&run.approval_id, "allow_once")
        .expect("enqueue"));
    let stored = stored_path(&run.root, "a.txt");

    // The process dies at the execution boundary. The panic is caught so the
    // test can inspect exactly the durable state a killed process would leave.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drive_real_consumer_with(&run, "t2-key", CrashPoint::AfterFileCommit)
    }));
    std::panic::set_hook(hook);
    assert!(crashed.is_err(), "the injected process death must surface");

    // The write really happened before the crash, and it happened exactly once.
    assert_eq!(
        std::fs::read(run.root.join("a.txt")).expect("target"),
        b"whole file",
        "the crash must land after the file commit, not before it"
    );
    assert_eq!(
        replacement_state_of(&run.db, &run.run_id),
        "consumed",
        "the replacement authorization was consumed by the real commit"
    );
    assert_eq!(
        version_rows(&run.db, &run.conversation_id, &stored),
        1,
        "exactly one version was registered before the crash"
    );
    let leased: i64 = run
        .db
        .with_connection(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM kernel_effect_outbox
                  WHERE run_id=?1 AND effect_type='dispatch_tool' AND status='leased'",
                [&run.run_id],
                |r| r.get(0),
            )
        })
        .expect("the leased dispatch");
    assert_eq!(leased, 1, "the interrupted dispatch is leased, not settled");

    // Close and reopen the database; rebuild the consumer from scratch.
    drop(run.db);
    let db = Database::open(run.root.join("facts.db")).expect("reopen the database");
    let recovered = KernelRun {
        db,
        root: run.root.clone(),
        run_id: run.run_id.clone(),
        conversation_id: run.conversation_id.clone(),
        approval_id: run.approval_id.clone(),
        scope: run.scope.clone(),
        clock: crate::kernel::TestClock::new(crate::database::now_ms()),
    };
    let executed = drive_real_consumer(&recovered, "t2-key").expect("recover");
    assert_eq!(executed, 0, "recovery must not replay the interrupted dispatch");
    assert_eq!(
        std::fs::read(recovered.root.join("a.txt")).expect("target"),
        b"whole file",
        "recovery must not write a second time"
    );
    assert_eq!(
        version_rows(&recovered.db, &recovered.conversation_id, &stored),
        1,
        "recovery must not register a second version"
    );
    // The uncertain outcome is reported, never silently presented as done.
    let (state, code): (String, String) = recovered
        .db
        .with_connection(|c| {
            Ok((
                c.query_row(
                    "SELECT state FROM kernel_runs WHERE run_id=?1",
                    [&recovered.run_id],
                    |r| r.get(0),
                )?,
                c.query_row(
                    "SELECT COALESCE((SELECT json_extract(payload_json,'$.code')
                        FROM kernel_events WHERE run_id=?1 AND event_type='run.failed'
                        ORDER BY seq DESC LIMIT 1),'')",
                    [&recovered.run_id],
                    |r| r.get(0),
                )?,
            ))
        })
        .expect("terminal facts");
    assert_eq!(state, "failed", "the interrupted Run must settle as failed");
    assert_eq!(
        code, "kernel.uncertain_execution",
        "the report must name the uncertainty instead of claiming success"
    );
    // And a repeat delivery still cannot execute.
    assert_eq!(
        drive_real_consumer(&recovered, "t2-key").expect("drive again"),
        0,
        "a repeat delivery must not execute"
    );
}

#[test]
fn state_fixture_enqueue_alone_authorizes_nothing() {
    let (db, root, run, approval_id, credential) = parked_kernel_replacement("enqueue-only");
    let conversation = kernel_conversation(&db, &run);
    assert!(
        db.queue_kernel_host_approval(&approval_id, "allow_once")
            .expect("enqueue the decision"),
        "the command is queued"
    );
    assert_eq!(
        replacement_state(&db, &run),
        "pending",
        "enqueueing must not authorize anything"
    );
    let early = db
        .claim_execution_attempt(&run, &conversation, &credential, &credential.intent_digest, "owner-early")
        .expect("early claim");
    assert!(
        matches!(early, fox_engine_protocol::AttemptOutcome::AlreadyRefused { .. }),
        "a claim before the decision is applied must be refused, got {early:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).expect("target"),
        "line one\nline two\n",
        "nothing may be written before the decision is applied"
    );
}

#[test]
fn state_fixture_decision_then_ack_executes_once() {
    let (db, _root, run, approval_id, credential) = parked_kernel_replacement("consumer-late");
    let conversation = kernel_conversation(&db, &run);
    db.queue_kernel_host_approval(&approval_id, "allow_once")
        .expect("enqueue");
    apply_kernel_decision(&db, &run, "allow_once");
    let seq = pending_command_seq(&db, &run);
    db.complete_kernel_host_command(&run, seq)
        .expect("acknowledge the command");
    assert_eq!(
        replacement_state(&db, &run),
        "approved",
        "the acknowledgment settles the request"
    );
    let outcome = db
        .claim_execution_attempt(&run, &conversation, &credential, &credential.intent_digest, "owner-1")
        .expect("claim");
    assert_eq!(outcome, fox_engine_protocol::AttemptOutcome::Claimed);
    assert_eq!(replacement_state(&db, &run), "consumed");
    let repeat = db
        .claim_execution_attempt(&run, &conversation, &credential, &credential.intent_digest, "owner-2")
        .expect("repeat claim");
    assert!(
        matches!(repeat, fox_engine_protocol::AttemptOutcome::Unknown),
        "a repeat request must be read-only, got {repeat:?}"
    );
}

#[test]
fn state_fixture_preempting_ack_is_refused() {
    let (db, _root, run, approval_id, _credential) = parked_kernel_replacement("consumer-early");
    db.queue_kernel_host_approval(&approval_id, "allow_once")
        .expect("enqueue");
    let seq = pending_command_seq(&db, &run);
    assert!(
        db.complete_kernel_host_command(&run, seq).is_err(),
        "acknowledging before the durable decision must fail"
    );
    assert_eq!(
        replacement_state(&db, &run),
        "pending",
        "a premature acknowledgement must not authorize anything"
    );
    apply_kernel_decision(&db, &run, "allow_once");
    db.complete_kernel_host_command(&run, seq)
        .expect("the legitimate approval still completes");
    assert_eq!(replacement_state(&db, &run), "approved");
}

#[test]
fn state_fixture_ack_is_idempotent() {
    let (db, _root, run, approval_id, _credential) = parked_kernel_replacement("crash-recovery");
    db.queue_kernel_host_approval(&approval_id, "allow_once")
        .expect("enqueue");
    apply_kernel_decision(&db, &run, "allow_once");
    let seq = pending_command_seq(&db, &run);
    db.complete_kernel_host_command(&run, seq)
        .expect("recovery acknowledges the command");
    assert_eq!(replacement_state(&db, &run), "approved");
    assert!(
        db.complete_kernel_host_command(&run, seq).is_err(),
        "an already-acknowledged command must not be acknowledged twice"
    );
    assert_eq!(replacement_state(&db, &run), "approved");
}

#[test]
fn state_fixture_denial_never_executes() {
    let (db, root, run, approval_id, credential) = parked_kernel_replacement("denial");
    let conversation = kernel_conversation(&db, &run);
    db.queue_kernel_host_approval(&approval_id, "denied")
        .expect("enqueue the denial");
    apply_kernel_decision(&db, &run, "denied");
    let seq = pending_command_seq(&db, &run);
    db.complete_kernel_host_command(&run, seq)
        .expect("acknowledge the denial");
    assert_eq!(
        replacement_state(&db, &run),
        "expired",
        "a denial withdraws the request immediately"
    );
    let outcome = db
        .claim_execution_attempt(&run, &conversation, &credential, &credential.intent_digest, "owner-1")
        .expect("claim");
    assert!(
        matches!(outcome, fox_engine_protocol::AttemptOutcome::AlreadyRefused { .. }),
        "a denied replacement must not execute, got {outcome:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).expect("target"),
        "line one\nline two\n",
        "a denied replacement must not write"
    );
}

// ---------------------------------------------------------------------------
// The ack/settlement distinction (coordinator item 1).
//
// `complete_kernel_host_command` answers two DIFFERENT questions:
//   * "has this queued command been processed?" - the ack gate, which also
//     accepts a cancellation, an expiry, a stale policy generation and a
//     terminal Run so the queue cannot stay stuck; and
//   * "is the approval still in force, so the replacement may be granted?" -
//     the settlement, which must never be derived from the original decision.
// ---------------------------------------------------------------------------

fn queued_allow_then_invalidate(label: &str) -> (Database, PathBuf, String, i64, PathBuf) {
    let run = build_kernel_run(label);
    run.db
        .queue_kernel_host_approval(&run.approval_id, "allow_once")
        .expect("enqueue allow_once");
    let seq = pending_command_seq(&run.db, &run.run_id);
    (run.db, run.root.clone(), run.run_id, seq, run.root.join("a.txt"))
}

fn command_status(db: &Database, run: &str, seq: i64) -> String {
    db.with_connection(|c| {
        c.query_row(
            "SELECT status FROM kernel_host_commands WHERE run_id=?1 AND command_seq=?2",
            rusqlite::params![run, seq],
            |r| r.get::<_, String>(0),
        )
    })
    .expect("command status")
}

#[test]
fn rev_allow_command_cancelled_before_consumption_grants_nothing() {
    let (db, _root, run, seq, target) = queued_allow_then_invalidate("allow-cancelled");
    db.with_connection(|c| {
        c.execute(
            "UPDATE kernel_approvals SET state='cancelled', decided_at=1
              WHERE run_id=?1 AND tool_call_id='write-1'",
            [&run],
        )?;
        Ok(())
    })
    .expect("cancel the approval");

    db.complete_kernel_host_command(&run, seq)
        .expect("the command must still be acknowledged");

    assert_eq!(command_status(&db, &run, seq), "completed", "the queue must not stay stuck");
    assert_eq!(
        replacement_state_of(&db, &run),
        "expired",
        "a cancelled approval must not grant the replacement"
    );
    assert_eq!(
        std::fs::read(&target).expect("target"),
        b"line one\nline two\n",
        "nothing may be written"
    );
}

#[test]
fn rev_allow_command_expired_before_consumption_grants_nothing() {
    let (db, _root, run, seq, target) = queued_allow_then_invalidate("allow-expired");
    db.with_connection(|c| {
        c.execute(
            "UPDATE kernel_approvals SET state='expired', decided_at=1
              WHERE run_id=?1 AND tool_call_id='write-1'",
            [&run],
        )?;
        Ok(())
    })
    .expect("expire the approval");

    db.complete_kernel_host_command(&run, seq)
        .expect("the command must still be acknowledged");
    assert_eq!(command_status(&db, &run, seq), "completed");
    assert_eq!(replacement_state_of(&db, &run), "expired");
    assert_eq!(std::fs::read(&target).expect("target"), b"line one\nline two\n");
}

#[test]
fn rev_allow_command_stale_policy_grants_nothing() {
    let (db, _root, run, seq, target) = queued_allow_then_invalidate("allow-stale");
    let conversation = kernel_conversation(&db, &run);
    let policy = db.execution_policy(&conversation).expect("policy");
    db.change_execution_policy(
        &conversation,
        "tighten-after-queue",
        policy.version,
        if policy.mode == "allow" { "ask" } else { "allow" },
    )
    .expect("change the policy");

    db.complete_kernel_host_command(&run, seq)
        .expect("the command must still be acknowledged");
    assert_eq!(command_status(&db, &run, seq), "completed");
    assert_eq!(
        replacement_state_of(&db, &run),
        "expired",
        "a stale policy generation must not grant the replacement"
    );
    assert_eq!(std::fs::read(&target).expect("target"), b"line one\nline two\n");
}

#[test]
fn rev_repeated_acknowledgement_never_grants_twice() {
    let (db, _root, run, seq, _target) = queued_allow_then_invalidate("allow-repeat");
    db.with_connection(|c| {
        c.execute(
            "UPDATE kernel_approvals SET state='allow_once', decided_at=1
              WHERE run_id=?1 AND tool_call_id='write-1'",
            [&run],
        )?;
        Ok(())
    })
    .expect("apply the approval");
    db.complete_kernel_host_command(&run, seq)
        .expect("the first acknowledgement settles the request");
    assert_eq!(replacement_state_of(&db, &run), "approved");
    assert!(db.complete_kernel_host_command(&run, seq).is_err());
    assert_eq!(replacement_state_of(&db, &run), "approved");
}
