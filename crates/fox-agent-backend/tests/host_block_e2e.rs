//! 完全访问下删除项目根的 Host 拦截（不依赖模型）。
//!
//! 假 Agent（`tests/fixtures/mock-acp-permission-probe.mjs`）在 `session/prompt`
//! 里直接发 ACP `session/request_permission`。Sink 按桌面 `external_policy.rs`
//! 的同等逻辑调用下面编进来的**桌面端真实引擎**
//! （`approval_policy::decide`）：Pi 整表判定；Codex / dsh 只在 `is_block()`
//! 时拒绝。拒绝选项写回 Agent，不出审批卡。
//!
//! 这里的审计是测试 sink 的内存审计，桌面端 `permission_audit` 表的真实写入由
//! 桌面 crate 的测试覆盖（另一个分支在 `external_policy.rs` 里加数据库级测试）。
//!
//! 没有 node 时跳过 ACP 往返（打印 SKIPPED）。环境变量 `CI` 存在时没有 node
//! 则失败，不能静默跳过。Windows 盘符路径和 POSIX 路径两边都跑，没有
//! `cfg(unix)` / `cfg(windows)`。
#![cfg(feature = "acp")]

// 引擎内部测试模块会一起编译运行。dead_code 只在这个测试 crate 里放行，不改引擎。
#[allow(dead_code)]
#[path = "../../../apps/desktop/src-tauri/src/runtime_host/approval_policy.rs"]
mod approval_policy;

use approval_policy::{decide, AccessMode, PolicyDecision, PolicyRequest, ToolSubject, Verdict};
use fox_agent_backend::acp::AcpConnection;
use fox_agent_backend::backends::pi::parse_tool_approval;
use fox_agent_backend::capability_view::PiLaunch;
use fox_agent_backend::external::{ExternalLauncher, ExternalRuntime, ExternalSink, HostPermissionAnswer, LaunchFailure, LaunchRequest, Launched};
use fox_agent_backend::external_home::{LaunchDeclaration, PermissionMode};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::native_permission::{self, FoxTier};
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::projection::{allow_option, reject_option, AutoPermission, PermissionCard, PermissionOption};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WIN_ROOT: &str = r"D:\fox-test\测试项目";
const WIN_HOME: &str = r"C:\Users\fox";
const POSIX_ROOT: &str = "/work/fox-e2e/proj";
const POSIX_HOME: &str = "/home/fox";

/// 与桌面 `pi_access_mode` / `external_access_mode` 相同：按会话 native mode
/// 的 Fox 档位。`full_access` 和 Codex 的 `agent-full-access` 都是完全访问。
fn access_mode(backend: AgentBackend, native_mode: &str) -> AccessMode {
    match native_permission::tier_of(backend, native_mode) {
        Some(FoxTier::FullAccess) => AccessMode::FullAccess,
        Some(FoxTier::ReadOnly) => AccessMode::ReadOnly,
        Some(FoxTier::Auto) => AccessMode::Auto,
        Some(FoxTier::Ask) | None => AccessMode::Ask,
    }
}

fn judge(command: &str, root: &Path, home: &Path, mode: AccessMode) -> PolicyDecision {
    let subject = ToolSubject::Command { command: command.to_owned(), cwd: root.to_path_buf(), model_risk: None };
    decide(&PolicyRequest { tool: "run_command", subject: &subject, mode, project_root: Some(root), home: Some(home), rules: &[] })
}

fn verdict_text(decision: &PolicyDecision) -> String {
    match &decision.verdict {
        Verdict::Allow => format!("allow class={} findings={:?}", decision.class.as_str(), decision.findings),
        Verdict::Ask => format!("ask class={} findings={:?}", decision.class.as_str(), decision.findings),
        Verdict::Deny { code, message } => format!("deny {code} class={} findings={:?} message={message}", decision.class.as_str(), decision.findings),
    }
}

fn windows_block_commands() -> &'static [&'static str] {
    &[
        r#"cmd //c "rd /s /q D:\fox-test\测试项目""#,
        r"cmd /c rmdir /s /q D:\fox-test\测试项目",
        "rm -rf D:/fox-test/测试项目",
        "rm -rf /d/fox-test/测试项目",
        "cd .. && rd /s /q 测试项目",
        r#"cd "D:\fox-test\测试项目" && rd /s /q D:\fox-test\测试项目 2>&1; echo "EXIT_CODE: $?""#,
        r#"cd "D:\fox-test\测试项目" && cd .. && rm -rf 测试项目 2>&1; echo "[exit:$?]""#,
    ]
}

fn posix_block_commands() -> &'static [&'static str] {
    &[
        "rm -rf /work/fox-e2e/proj",
        "cd .. && rm -rf proj",
        r#"cd "/work/fox-e2e/proj" && rm -rf . 2>&1; echo $?"#,
    ]
}

/// 引擎单独判定（不启动 node）。某条没被拒时断言失败，并带上引擎的判定。
#[test]
fn included_engine_blocks_project_root_deletes_and_allows_inside_the_project() {
    let win = PathBuf::from(WIN_ROOT);
    let win_home = PathBuf::from(WIN_HOME);
    for command in windows_block_commands() {
        let decision = judge(command, &win, &win_home, AccessMode::FullAccess);
        assert!(decision.is_block(), "{command}: {}", verdict_text(&decision));
        assert!(decision.findings.iter().any(|finding| finding == "builtin_block"), "{command}: {:?}", decision.findings);
        assert_eq!(decision.mode.as_str(), "full_access");
        assert_eq!(decision.class.as_str(), "command");
    }
    let posix = PathBuf::from(POSIX_ROOT);
    let posix_home = PathBuf::from(POSIX_HOME);
    for command in posix_block_commands() {
        let decision = judge(command, &posix, &posix_home, AccessMode::FullAccess);
        assert!(decision.is_block(), "{command}: {}", verdict_text(&decision));
        assert!(decision.findings.iter().any(|finding| finding == "builtin_block"), "{command}: {:?}", decision.findings);
    }
    // Codex 数组按空格拼接后交给同一引擎：`bash -lc rm -rf <根>`。
    let joined = "bash -lc rm -rf /work/fox-e2e/proj";
    let decision = judge(joined, &posix, &posix_home, AccessMode::FullAccess);
    assert!(decision.is_block(), "{joined}: {}", verdict_text(&decision));
    for command in ["dir", "rm -rf build"] {
        let decision = judge(command, &win, &win_home, AccessMode::FullAccess);
        assert!(!decision.is_block(), "{command}: {}", verdict_text(&decision));
        assert!(decision.is_allow(), "{command}: {}", verdict_text(&decision));
    }
}

/// 与桌面 `record_permission_audit` 写入的列一致（拒绝码在 reason）。
#[derive(Clone, Debug)]
struct AuditRow {
    tool: String,
    mode: String,
    class: String,
    risk: String,
    reason: String,
    findings: Vec<String>,
}

struct Recorded {
    events: Vec<(String, Value)>,
    approvals: Vec<(String, PermissionCard)>,
    audits: Vec<AuditRow>,
    project_root: PathBuf,
    home: PathBuf,
}

#[derive(Clone)]
struct Sink(Arc<Mutex<Recorded>>);

impl Sink {
    fn new(project_root: PathBuf, home: PathBuf) -> Self {
        Self(Arc::new(Mutex::new(Recorded { events: Vec::new(), approvals: Vec::new(), audits: Vec::new(), project_root, home })))
    }

    fn set_paths(&self, project_root: PathBuf, home: PathBuf) {
        let mut recorded = self.0.lock().unwrap();
        recorded.project_root = project_root;
        recorded.home = home;
    }

    fn events(&self, run_id: &str) -> Vec<Value> {
        self.0.lock().unwrap().events.iter().filter(|(run, _)| run == run_id).map(|(_, event)| event.clone()).collect()
    }

    fn text(&self, run_id: &str) -> String {
        self.events(run_id).iter().filter(|event| event["type"] == "message.delta").map(|event| event["delta"].as_str().unwrap_or("").to_owned()).collect()
    }

    fn approval_len(&self) -> usize {
        self.0.lock().unwrap().approvals.len()
    }

    fn audit_len(&self) -> usize {
        self.0.lock().unwrap().audits.len()
    }

    fn audits(&self) -> Vec<AuditRow> {
        self.0.lock().unwrap().audits.clone()
    }

    async fn terminal(&self, run_id: &str) -> Value {
        for _ in 0..200 {
            if let Some(event) = self.events(run_id).into_iter().find(|event| {
                event["type"].as_str().is_some_and(|kind| matches!(kind, "run.completed" | "run.cancelled" | "run.failed" | "run.interrupted"))
            }) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no terminal for {run_id}; events {:?}", self.events(run_id));
    }
}

/// `toolCall.rawInput.command`：字符串，或字符串数组按空格拼接。
fn command_line(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        let text = text.trim();
        return (!text.is_empty()).then(|| text.to_owned());
    }
    let words: Vec<&str> = value.as_array()?.iter().filter_map(Value::as_str).map(str::trim).filter(|word| !word.is_empty()).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn pi_subject(request: &Value, project_root: &Path) -> (String, ToolSubject) {
    let Some(approval) = parse_tool_approval(request) else {
        return ("agent_permission".to_owned(), ToolSubject::Other);
    };
    if approval.tool_name != "bash" {
        return (approval.tool_name, ToolSubject::Other);
    }
    let cwd = approval
        .cwd
        .as_deref()
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.to_path_buf());
    match approval.input.get("command").and_then(Value::as_str).map(str::trim).filter(|command| !command.is_empty()) {
        Some(command) => (
            "run_command".to_owned(),
            ToolSubject::Command {
                command: command.to_owned(),
                cwd,
                model_risk: approval.input.get("risk").and_then(Value::as_str).map(str::to_owned),
            },
        ),
        None => ("run_command".to_owned(), ToolSubject::Other),
    }
}

fn native_command(request: &Value, project_root: &Path) -> Option<ToolSubject> {
    let raw = request.get("toolCall")?.get("rawInput")?;
    let command = command_line(raw.get("command")?)?;
    let cwd = raw
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.to_path_buf());
    Some(ToolSubject::Command { command, cwd, model_risk: None })
}

fn finish(tool: &str, decision: &PolicyDecision, options: &[PermissionOption], audits: &mut Vec<AuditRow>) -> HostPermissionAnswer {
    let auto = match &decision.verdict {
        Verdict::Allow => allow_option(options)
            .map(|option| AutoPermission::Allow { option_id: option.option_id.clone() })
            .unwrap_or_else(|| AutoPermission::Reject { option_id: reject_option(options).map(|option| option.option_id.clone()) }),
        Verdict::Ask => AutoPermission::Ask,
        Verdict::Deny { code, .. } => {
            // 拒绝才记一行。reason 是拒绝码，与桌面表的 reason 列相同。
            audits.push(AuditRow {
                tool: tool.to_owned(),
                mode: decision.mode.as_str().to_owned(),
                class: decision.class.as_str().to_owned(),
                risk: decision.risk.as_str().to_owned(),
                reason: code.clone(),
                findings: decision.findings.clone(),
            });
            AutoPermission::Reject { option_id: reject_option(options).map(|option| option.option_id.clone()) }
        }
    };
    let mut policy = decision.card_json();
    policy["tool"] = json!(tool);
    if let Verdict::Deny { code, .. } = &decision.verdict {
        policy["verdict"] = json!("deny");
        policy["denyCode"] = json!(code);
    }
    HostPermissionAnswer { decision: auto, policy }
}

impl ExternalSink for Sink {
    fn emit(&self, run_id: &str, _conversation_id: &str, event: Value) -> Result<(), String> {
        self.0.lock().unwrap().events.push((run_id.to_owned(), event));
        Ok(())
    }

    fn request_approval(&self, run_id: &str, _conversation_id: &str, _key: &str, card: &PermissionCard) -> Result<String, String> {
        let mut recorded = self.0.lock().unwrap();
        let id = format!("appr-{}", recorded.approvals.len() + 1);
        recorded.approvals.push((run_id.to_owned(), card.clone()));
        Ok(id)
    }

    fn host_permission_answer(
        &self,
        _run_id: &str,
        _conversation_id: &str,
        backend: AgentBackend,
        native_mode: &str,
        request: &Value,
        options: &[PermissionOption],
    ) -> Option<HostPermissionAnswer> {
        let mut recorded = self.0.lock().unwrap();
        let root = recorded.project_root.clone();
        let home = recorded.home.clone();
        let mode = access_mode(backend, native_mode);
        if backend == AgentBackend::Pi {
            let (tool, subject) = pi_subject(request, &root);
            let decision = decide(&PolicyRequest { tool: &tool, subject: &subject, mode, project_root: Some(&root), home: Some(&home), rules: &[] });
            return Some(finish(&tool, &decision, options, &mut recorded.audits));
        }
        if matches!(backend, AgentBackend::Codex | AgentBackend::Deepseek) {
            let Some(subject) = native_command(request, &root) else { return None };
            let decision = decide(&PolicyRequest { tool: "run_command", subject: &subject, mode, project_root: Some(&root), home: Some(&home), rules: &[] });
            // 没有命中 block 就不替 Codex / dsh 作答。
            if !decision.is_block() {
                return None;
            }
            return Some(finish("run_command", &decision, options, &mut recorded.audits));
        }
        None
    }
}

#[derive(Clone, Default)]
struct ProbeLauncher;

impl ExternalLauncher for ProbeLauncher {
    fn desired_mode(&self, request: &LaunchRequest) -> PermissionMode {
        if native_permission::is_full_access(request.backend, request.native_mode()) { PermissionMode::FullAccess } else { PermissionMode::Standard }
    }

    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let mode = self.desired_mode(&request);
        Box::pin(async move {
            let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock-acp-permission-probe.mjs");
            let spec = LaunchSpec {
                backend: request.backend,
                program: "node".into(),
                args: vec![script.into()],
                version_pin: "mock",
                credential_env: &[],
                experimental: false,
                session_env: vec![],
            };
            // 盘符项目根在 Linux 上不存在。进程 cwd 用临时目录；引擎看的是
            // 探针写进 request_permission 的 cwd，以及 sink 上的项目根。
            let connection = AcpConnection::spawn_detailed(&spec, &std::env::temp_dir()).await.map_err(LaunchFailure::from)?;
            Ok(Launched {
                connection,
                mode,
                declaration: LaunchDeclaration::declare(mode),
                pi_launch: PiLaunch::Stock,
                on_closed: None,
                fox_tools: None,
            })
        })
    }
}

fn require_node() -> bool {
    if std::process::Command::new("node").arg("--version").output().is_ok() {
        return true;
    }
    if std::env::var_os("CI").is_some() {
        panic!("CI 环境变量已设置但没有 node，host_block_e2e 不能跳过");
    }
    eprintln!("SKIPPED: node not installed");
    false
}

fn turn(conversation: &str, run: &str, prompt: &str, backend: AgentBackend, native: &str) -> fox_agent_backend::external::TurnRequest {
    fox_agent_backend::external::TurnRequest {
        run_id: run.into(),
        prompt: prompt.into(),
        stored_native_session: None,
        launch: LaunchRequest {
            conversation_id: conversation.into(),
            backend,
            project_uuid: Some("8f14e45f-ceea-467a-9a36-dedd4bea2543".into()),
            project_root: std::env::temp_dir(),
            session_key: format!("k-{run}"),
            full_access_granted: true,
            unified_login: None,
            native_permission: Some(native.to_owned()),
        },
    }
}

fn prompt_of(shape: &str, command: Value, cwd: &str) -> String {
    json!({ "shape": shape, "command": command, "cwd": cwd }).to_string()
}

fn assert_blocked(sink: &Sink, run: &str, command: &str, before_audits: usize, before_cards: usize, echo: &str) {
    let text = sink.text(run);
    let answered = sink.events(run).into_iter().find(|event| event["type"] == "agent_backend.permission_auto_answered");
    assert_eq!(text, echo, "{command}: 回显 {text:?}，自动应答 {answered:?}");
    assert_eq!(sink.approval_len(), before_cards, "{command}: 出了审批卡");
    let audits = sink.audits();
    assert_eq!(audits.len(), before_audits + 1, "{command}: 审计条数 {:?} → {:?}", audits, before_audits);
    let row = audits.last().expect("审计");
    assert_eq!(row.reason, "permission.blocked", "{command}: {row:?}");
    assert!(row.findings.iter().any(|finding| finding == "builtin_block"), "{command}: {row:?}");
    assert_eq!(row.mode, "full_access", "{command}: {row:?}");
    assert_eq!(row.tool, "run_command", "{command}: {row:?}");
    assert_eq!(row.class, "command", "{command}: block 仍是命令类 {row:?}");
    assert_eq!(row.risk, "high", "{command}: {row:?}");
    let answered = answered.expect("agent_backend.permission_auto_answered");
    assert_eq!(answered["decision"], "reject", "{command}: {answered}");
    assert_eq!(answered["answeredBy"], "host_approval_policy", "{command}: {answered}");
}

async fn run_case(runtime: &ExternalRuntime<Sink, ProbeLauncher>, sink: &Sink, conversation: &str, run: &str, backend: AgentBackend, native: &str, shape: &str, command: Value, cwd: &str, home: &str) {
    sink.set_paths(PathBuf::from(cwd), PathBuf::from(home));
    runtime.start_turn(turn(conversation, run, &prompt_of(shape, command, cwd), backend, native)).expect("start");
    let terminal = sink.terminal(run).await;
    assert_eq!(terminal["type"], "run.completed", "{run}: {terminal} text {}", sink.text(run));
}

#[tokio::test(flavor = "multi_thread")]
async fn full_access_pi_rejects_project_root_deletes_without_a_card() {
    if !require_node() {
        return;
    }
    let sink = Sink::new(PathBuf::from(WIN_ROOT), PathBuf::from(WIN_HOME));
    let runtime = ExternalRuntime::with_cancel_timeout(sink.clone(), ProbeLauncher, tokio::runtime::Handle::current(), Duration::from_millis(800));
    let mut index = 0usize;
    for command in windows_block_commands() {
        let run = format!("pi-win-{index}");
        index += 1;
        let before_audits = sink.audit_len();
        let before_cards = sink.approval_len();
        run_case(&runtime, &sink, "pi-block", &run, AgentBackend::Pi, "full_access", "pi", json!(command), WIN_ROOT, WIN_HOME).await;
        // Pi 的拒绝选项 id 是 no。
        assert_blocked(&sink, &run, command, before_audits, before_cards, "permission:no");
    }
    for command in posix_block_commands() {
        let run = format!("pi-posix-{index}");
        index += 1;
        let before_audits = sink.audit_len();
        let before_cards = sink.approval_len();
        run_case(&runtime, &sink, "pi-block", &run, AgentBackend::Pi, "full_access", "pi", json!(command), POSIX_ROOT, POSIX_HOME).await;
        assert_blocked(&sink, &run, command, before_audits, before_cards, "permission:no");
    }
    // 对照：完全访问下项目内命令放行，审计不再增加 block 记录，也不出卡。
    for command in ["dir", "rm -rf build"] {
        let run = format!("pi-allow-{index}");
        index += 1;
        let before_audits = sink.audit_len();
        let before_cards = sink.approval_len();
        run_case(&runtime, &sink, "pi-block", &run, AgentBackend::Pi, "full_access", "pi", json!(command), WIN_ROOT, WIN_HOME).await;
        assert_eq!(sink.text(&run), "permission:yes", "{command}");
        assert_eq!(sink.approval_len(), before_cards, "{command}: 出了审批卡");
        assert_eq!(sink.audit_len(), before_audits, "{command}: 不该有新的 block 审计 {:?}", sink.audits());
        let answered = sink.events(&run).into_iter().find(|event| event["type"] == "agent_backend.permission_auto_answered").expect("answered");
        assert_eq!(answered["decision"], "allow_once", "{command}: {answered}");
    }
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn full_access_codex_rejects_root_delete_string_and_bash_array_without_a_card() {
    if !require_node() {
        return;
    }
    let sink = Sink::new(PathBuf::from(POSIX_ROOT), PathBuf::from(POSIX_HOME));
    let runtime = ExternalRuntime::with_cancel_timeout(sink.clone(), ProbeLauncher, tokio::runtime::Handle::current(), Duration::from_millis(800));
    let cases = [
        json!(format!("rm -rf {POSIX_ROOT}")),
        json!(["bash", "-lc", format!("rm -rf {POSIX_ROOT}")]),
    ];
    for (index, command) in cases.into_iter().enumerate() {
        let run = format!("codex-{index}");
        let label = command.to_string();
        let before_audits = sink.audit_len();
        let before_cards = sink.approval_len();
        run_case(&runtime, &sink, "codex-block", &run, AgentBackend::Codex, "agent-full-access", "codex", command, POSIX_ROOT, POSIX_HOME).await;
        // Codex 的拒绝选项 id 是 reject_once（kind 同名），不是 pi 的 no。
        assert_blocked(&sink, &run, &label, before_audits, before_cards, "permission:reject_once");
    }
    runtime.shutdown().await;
}
