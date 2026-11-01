//! 同一个外部 ACP 会话按用户顺序走完：流式回复、审批出卡、真子进程工具、
//! 结果回写、同一进程的下一轮，然后取消并清扫后端故意留下的工具子进程，
//! 后端还在，再一轮仍能完成。pi、codex、deepseek 各跑一遍。
//!
//! Linux 与 Windows 都跑。没有 node 时打印 SKIPPED 并返回；CI 上没有 node 则失败。
#![cfg(feature = "acp")]

use fox_agent_backend::acp::AcpConnection;
use fox_agent_backend::capability_view::PiLaunch;
use fox_agent_backend::external::{
    AnswerOutcome, CancelOutcome, ExternalLauncher, ExternalRuntime, ExternalSink,
    HostPermissionAnswer, LaunchFailure, LaunchRequest, Launched, TurnRequest,
};
use fox_agent_backend::external_home::{LaunchDeclaration, PermissionMode};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::{AgentBackend, StopConfirmation};
use fox_agent_backend::proctree::process_ended;
use fox_agent_backend::projection::{AutoPermission, PermissionCard, PermissionOption};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);
/// 取消确认超时。清扫宽限期是 `CANCEL_SWEEP_GRACE`（1.5 秒），子进程消失的断言用 5 秒，留出余量。
const CANCEL_TIMEOUT: Duration = Duration::from_secs(3);
const CHILD_GONE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Recorded {
    events: Vec<(String, Value)>,
    approvals: Vec<(String, String, PermissionCard)>,
    /// (backend, native_mode, 是否由 Host 审批引擎返回了询问)。
    host_answers: Vec<(String, String, bool)>,
}

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Recorded>>);

impl ExternalSink for Sink {
    fn emit(&self, run_id: &str, _conversation_id: &str, event: Value) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .events
            .push((run_id.to_owned(), event));
        Ok(())
    }

    fn request_approval(
        &self,
        run_id: &str,
        _conversation_id: &str,
        key: &str,
        card: &PermissionCard,
    ) -> Result<String, String> {
        assert!(key.starts_with("acp-permission:"), "{key}");
        let mut recorded = self.0.lock().unwrap();
        let id = format!("appr-{}", recorded.approvals.len() + 1);
        recorded
            .approvals
            .push((run_id.to_owned(), id.clone(), card.clone()));
        Ok(id)
    }

    fn host_permission_answer(
        &self,
        _run_id: &str,
        _conversation_id: &str,
        backend: AgentBackend,
        native_mode: &str,
        _request: &Value,
        _options: &[PermissionOption],
    ) -> Option<HostPermissionAnswer> {
        // pi 由 Host 审批引擎判定为询问。codex / dsh 返回 None，交给本机档位表出卡。
        let via_host = backend == AgentBackend::Pi;
        self.0.lock().unwrap().host_answers.push((
            backend.as_str().to_owned(),
            native_mode.to_owned(),
            via_host,
        ));
        if via_host {
            Some(HostPermissionAnswer {
                decision: AutoPermission::Ask,
                policy: json!({ "mode": native_mode, "tier": "ask" }),
            })
        } else {
            None
        }
    }
}

impl Sink {
    fn events(&self, run_id: &str) -> Vec<Value> {
        self.0
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|(run, _)| run == run_id)
            .map(|(_, event)| event.clone())
            .collect()
    }

    fn types(&self, run_id: &str) -> Vec<String> {
        self.events(run_id)
            .iter()
            .map(|event| event["type"].as_str().unwrap_or("?").to_owned())
            .collect()
    }

    fn text(&self, run_id: &str) -> String {
        self.events(run_id)
            .iter()
            .filter(|event| event["type"] == "message.delta")
            .map(|event| event["delta"].as_str().unwrap_or("").to_owned())
            .collect()
    }

    fn approvals(&self) -> Vec<(String, String, PermissionCard)> {
        self.0.lock().unwrap().approvals.clone()
    }

    fn host_answers(&self) -> Vec<(String, String, bool)> {
        self.0.lock().unwrap().host_answers.clone()
    }
}

#[derive(Clone, Default)]
struct MockLauncher {
    launches: Arc<AtomicUsize>,
}

impl ExternalLauncher for MockLauncher {
    fn desired_mode(&self, _request: &LaunchRequest) -> PermissionMode {
        PermissionMode::Standard
    }

    fn launch(
        &self,
        request: LaunchRequest,
    ) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let launches = Arc::clone(&self.launches);
        Box::pin(async move {
            launches.fetch_add(1, Ordering::SeqCst);
            let script = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/mock-acp-e2e-agent.mjs"
            );
            let spec = LaunchSpec {
                backend: request.backend,
                program: "node".into(),
                args: vec![script.into()],
                version_pin: "mock",
                credential_env: &[],
                experimental: false,
                session_env: vec![],
            };
            let connection = AcpConnection::spawn_detailed(&spec, &request.project_root)
                .await
                .map_err(LaunchFailure::from)?;
            Ok(Launched {
                connection,
                mode: PermissionMode::Standard,
                declaration: LaunchDeclaration::declare(PermissionMode::Standard),
                pi_launch: PiLaunch::Stock,
                on_closed: None,
                fox_tools: None,
            })
        })
    }
}

/// 没有 node：CI 上必须跑，否则跳过。
fn require_node() -> bool {
    if std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok()
    {
        return true;
    }
    if std::env::var_os("CI").is_some() {
        panic!("CI 上没有 node，外部 ACP 端到端测试不能跳过");
    }
    eprintln!("SKIPPED: node not installed");
    false
}

fn ask_mode(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Pi => "ask",
        AgentBackend::Codex | AgentBackend::Deepseek => "workspace-write",
        AgentBackend::Fox => panic!("本测试只覆盖 pi、codex、deepseek"),
    }
}

fn slug(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Pi => "pi",
        AgentBackend::Codex => "codex",
        AgentBackend::Deepseek => "dsh",
        AgentBackend::Fox => "fox",
    }
}

fn turn(conversation: &str, run: &str, prompt: &str, backend: AgentBackend) -> TurnRequest {
    TurnRequest {
        run_id: run.into(),
        prompt: prompt.into(),
        stored_native_session: None,
        launch: LaunchRequest {
            conversation_id: conversation.into(),
            backend,
            project_uuid: None,
            project_root: std::env::temp_dir(),
            session_key: format!("k-{run}"),
            full_access_granted: false,
            unified_login: None,
            native_permission: Some(ask_mode(backend).into()),
        },
    }
}

fn is_terminal(event: &Value) -> bool {
    event["type"].as_str().is_some_and(|kind| {
        matches!(
            kind,
            "run.completed" | "run.cancelled" | "run.failed" | "run.interrupted"
        )
    })
}

fn event_index(events: &[Value], kind: &str) -> usize {
    events
        .iter()
        .position(|event| event["type"] == kind)
        .unwrap_or_else(|| panic!("缺少 {kind}，已收到 {:?}", types_of(events)))
}

fn types_of(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap_or("?").to_owned())
        .collect()
}

/// `expected` 按这个顺序出现（中间可以有别的事件）。
fn assert_order(events: &[Value], expected: &[&str]) {
    let mut from = 0;
    for kind in expected {
        let relative = events
            .iter()
            .skip(from)
            .position(|event| event["type"] == *kind)
            .unwrap_or_else(|| {
                panic!(
                    "在位置 {from} 之后缺少 {kind}，已收到 {:?}",
                    types_of(events)
                )
            });
        from += relative + 1;
    }
}

async fn wait_until(
    timeout: Duration,
    what: &str,
    types: impl Fn() -> Vec<String>,
    mut ready: impl FnMut() -> bool,
) {
    let deadline = Instant::now() + timeout;
    loop {
        if ready() {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "{what} 超时（{} 秒），已收到事件: {:?}",
                timeout.as_secs(),
                types()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn labeled_pid(text: &str, label: &str) -> u32 {
    let rest = text
        .split(label)
        .nth(1)
        .unwrap_or_else(|| panic!("没有 {label}: {text}"));
    let digits: String = rest
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("{label} 的 pid「{digits}」无法解析: {error}；原文 {text}"))
}

/// 测试失败时把假 Agent 故意留下的子进程杀掉，避免泄漏。
struct Reap(Vec<u32>);

impl Drop for Reap {
    fn drop(&mut self) {
        for pid in &self.0 {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }
}

async fn run_full_turn(backend: AgentBackend) {
    if !require_node() {
        return;
    }
    let name = slug(backend);
    let conversation = format!("e2e-{name}");
    let run1 = format!("{name}-1");
    let run2 = format!("{name}-2");
    let run3 = format!("{name}-3");
    let run4 = format!("{name}-4");
    let sink = Sink::default();
    let launcher = MockLauncher::default();
    let runtime = ExternalRuntime::with_cancel_timeout(
        sink.clone(),
        launcher.clone(),
        tokio::runtime::Handle::current(),
        CANCEL_TIMEOUT,
    );

    runtime
        .start_turn(turn(&conversation, &run1, "e2e-tool", backend))
        .unwrap();
    let (approval_run, approval_id, card) = {
        let sink = sink.clone();
        let run1 = run1.clone();
        let mut found = None;
        wait_until(
            WAIT,
            "第 1 轮审批卡",
            || sink.types(&run1),
            || {
                let approvals = sink.approvals();
                if approvals.len() == 1 {
                    found = Some(approvals.into_iter().next().unwrap());
                    true
                } else {
                    false
                }
            },
        )
        .await;
        found.unwrap()
    };
    assert_eq!(approval_run, run1);
    assert_eq!(
        card.available_decisions,
        ["allow_once", "deny"],
        "审批卡只提供允许一次和拒绝: {card:?}"
    );
    assert!(!card.available_decisions.contains(&"allow_always"));
    let host_answers = sink.host_answers();
    assert_eq!(
        host_answers.len(),
        1,
        "只应有一次审批询问: {host_answers:?}"
    );
    assert_eq!(
        host_answers[0].1,
        ask_mode(backend),
        "本轮应走该后端的询问档: {host_answers:?}"
    );
    if backend == AgentBackend::Pi {
        assert!(
            host_answers[0].2,
            "pi 应由 Host 审批引擎判定为询问: {host_answers:?}"
        );
    } else {
        assert!(
            !host_answers[0].2,
            "codex / dsh 不由 Host 引擎作答，交给本机询问档: {host_answers:?}"
        );
    }
    let before = sink.events(&run1);
    assert_order(
        &before,
        &[
            "run.started",
            "agent_backend.launching",
            "agent_backend.session",
            "message.delta",
            "tool.started",
        ],
    );
    assert_eq!(
        event_index(&before, "run.started"),
        0,
        "run.started 应是第 1 轮的第一个事件: {:?}",
        types_of(&before)
    );
    let streamed: String = before
        .iter()
        .take_while(|event| event["type"] != "tool.started")
        .filter(|event| event["type"] == "message.delta")
        .map(|event| event["delta"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        streamed, "先看工作区，再执行命令。",
        "流式回复应拼成完整的一句: {streamed}"
    );
    assert_eq!(
        before
            .iter()
            .filter(|event| event["type"] == "tool.started")
            .count(),
        1
    );
    assert_eq!(
        before[event_index(&before, "agent_backend.session")]["permissionMode"],
        ask_mode(backend)
    );
    let native_session = before[event_index(&before, "agent_backend.session")]["nativeSessionId"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(runtime.answer(&approval_id, true), AnswerOutcome::Written);
    {
        let sink = sink.clone();
        let run1 = run1.clone();
        wait_until(
            WAIT,
            "第 1 轮结束",
            || sink.types(&run1),
            || sink.events(&run1).iter().any(is_terminal),
        )
        .await;
    }
    let first = sink.events(&run1);
    let terminal = first.iter().find(|event| is_terminal(event)).unwrap();
    assert_eq!(terminal["type"], "run.completed", "{terminal}");
    assert_eq!(terminal["stopReason"], "end_turn", "{terminal}");
    assert_eq!(
        first.last().unwrap()["type"],
        "run.completed",
        "{:?}",
        types_of(&first)
    );
    let completed_at = event_index(&first, "tool.completed");
    assert!(event_index(&first, "tool.started") < completed_at);
    let completed = &first[completed_at];
    assert_eq!(completed["isError"], false, "{completed}");
    let result_text = completed["result"]["text"].as_str().unwrap_or("");
    assert!(
        result_text.contains("tool-ok"),
        "工具结果应带回子进程输出: {completed}"
    );
    let after_tool: String = first
        .iter()
        .skip(completed_at + 1)
        .filter(|event| event["type"] == "message.delta")
        .map(|event| event["delta"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        after_tool, "命令已完成。",
        "工具完成之后应有回复: {after_tool}"
    );
    assert_eq!(sink.approvals().len(), 1, "整轮只出一张审批卡");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);

    runtime
        .start_turn(turn(&conversation, &run2, "e2e-plain", backend))
        .unwrap();
    {
        let sink = sink.clone();
        let run2 = run2.clone();
        wait_until(
            WAIT,
            "第 2 轮结束",
            || sink.types(&run2),
            || sink.events(&run2).iter().any(is_terminal),
        )
        .await;
    }
    let second = sink.events(&run2);
    assert_eq!(
        second.iter().find(|event| is_terminal(event)).unwrap()["type"],
        "run.completed",
        "{second:?}"
    );
    assert_eq!(
        second.iter().find(|event| is_terminal(event)).unwrap()["stopReason"],
        "end_turn"
    );
    assert_eq!(sink.text(&run2), "同一会话的第二轮。");
    assert!(
        !second
            .iter()
            .any(|event| event["type"] == "agent_backend.session"
                || event["type"] == "agent_backend.launching"),
        "同一进程不应再次启动: {:?}",
        types_of(&second)
    );
    assert_eq!(
        launcher.launches.load(Ordering::SeqCst),
        1,
        "第 2 轮应复用已经启动的后端"
    );
    let info = runtime.session_info(&conversation).unwrap();
    assert!(info.live, "第 2 轮之后后端连接应仍在");
    assert_eq!(
        info.native_session_id.as_deref(),
        Some(native_session.as_str())
    );

    runtime
        .start_turn(turn(&conversation, &run3, "e2e-hold", backend))
        .unwrap();
    {
        let sink = sink.clone();
        let run3 = run3.clone();
        wait_until(
            WAIT,
            "第 3 轮回报子进程",
            || sink.types(&run3),
            || {
                let text = sink.text(&run3);
                if text.contains("spawn-failed") {
                    panic!(
                        "假 Agent 没能拉起长命子进程: {text} 事件={:?}",
                        sink.types(&run3)
                    );
                }
                text.contains("child-pid:") && text.contains("backend-pid:")
            },
        )
        .await;
    }
    let reported = sink.text(&run3);
    let child_pid = labeled_pid(&reported, "child-pid:");
    let backend_pid = labeled_pid(&reported, "backend-pid:");
    assert!(
        child_pid != backend_pid,
        "子进程应不同于后端进程: {reported}"
    );
    assert!(!process_ended(child_pid), "取消前子进程应仍在: {child_pid}");
    assert!(
        !process_ended(backend_pid),
        "取消前后端应仍在: {backend_pid}"
    );
    let _reap = Reap(vec![child_pid]);
    assert!(
        !sink
            .events(&run3)
            .iter()
            .any(|event| event["type"] == "agent_backend.session"),
        "第 3 轮不应新开会话: {:?}",
        sink.types(&run3)
    );

    let cancelled_at = Instant::now();
    assert!(
        matches!(runtime.cancel(&run3), CancelOutcome::Requested { .. }),
        "取消应已发给后端"
    );
    let pid_deadline = cancelled_at + CHILD_GONE;
    let event_deadline = cancelled_at + WAIT;
    loop {
        let gone = process_ended(child_pid);
        let events = sink.events(&run3);
        let saw_terminal = events.iter().any(is_terminal);
        if gone && saw_terminal {
            break;
        }
        if !gone && Instant::now() >= pid_deadline {
            panic!(
                "取消后 5 秒子进程仍在: pid={child_pid} 事件={:?}",
                sink.types(&run3)
            );
        }
        if !saw_terminal && Instant::now() >= event_deadline {
            panic!("没有等到第 3 轮结束，已收到事件: {:?}", sink.types(&run3));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let third = sink.events(&run3);
    let cancel_terminal = third.iter().find(|event| is_terminal(event)).unwrap();
    assert_eq!(
        cancel_terminal["type"], "run.cancelled",
        "{cancel_terminal}"
    );
    assert_eq!(
        cancel_terminal["stopReason"], "cancelled",
        "{cancel_terminal}"
    );
    assert!(
        cancel_terminal.get("outcome").is_none(),
        "后端确认了取消，不应是结果未知: {cancel_terminal}"
    );
    let swept_at = event_index(&third, "agent_backend.cancel_swept");
    let terminal_at = event_index(&third, "run.cancelled");
    assert!(
        swept_at < terminal_at,
        "cancel_swept 必须在 run.cancelled 之前: {:?}",
        types_of(&third)
    );
    let swept = &third[swept_at];
    // 13.37：清扫记录带上取消链路各段耗时，宽限期是固定的 CANCEL_SWEEP_GRACE。
    let timing = &swept["timing"];
    assert!(timing["confirmMs"].is_u64(), "cancel_swept 应记录确认耗时: {swept}");
    assert!(
        timing["graceMs"].as_u64().is_some_and(|grace| grace >= fox_agent_backend::external::CANCEL_SWEEP_GRACE.as_millis() as u64),
        "宽限期耗时应不短于 CANCEL_SWEEP_GRACE: {swept}"
    );
    assert!(timing["sweepMs"].is_u64(), "cancel_swept 应记录清扫耗时: {swept}");
    let killed_count = swept["killed"]["count"]
        .as_u64()
        .unwrap_or_else(|| panic!("killed.count 缺失: {swept}"));
    assert!(killed_count >= 1, "清扫应至少结束一个本轮进程: {swept}");
    let killed_pids: Vec<u64> = swept["killed"]["pids"]
        .as_array()
        .unwrap_or_else(|| panic!("killed.pids 缺失: {swept}"))
        .iter()
        .filter_map(|value| value.as_u64())
        .collect();
    assert!(
        killed_pids.contains(&u64::from(child_pid)),
        "清扫结果应包含这个子进程: {swept}"
    );
    assert!(
        swept["failed"]
            .as_array()
            .is_some_and(|failed| failed.is_empty()),
        "清扫失败必须停后端，不能记成成功: {swept}"
    );
    assert_eq!(swept["noBaseline"], false, "{swept}");
    assert!(process_ended(child_pid), "子进程应已不在: {child_pid}");
    assert!(
        !process_ended(backend_pid),
        "清扫之后后端进程本身应仍在: {backend_pid}"
    );
    assert!(
        runtime.session_info(&conversation).unwrap().live,
        "取消后会话连接应仍在"
    );
    assert_eq!(
        launcher.launches.load(Ordering::SeqCst),
        1,
        "取消清扫不应重启后端"
    );

    runtime
        .start_turn(turn(&conversation, &run4, "e2e-resume", backend))
        .unwrap();
    {
        let sink = sink.clone();
        let run4 = run4.clone();
        wait_until(
            WAIT,
            "第 4 轮结束",
            || sink.types(&run4),
            || sink.events(&run4).iter().any(is_terminal),
        )
        .await;
    }
    let fourth = sink.events(&run4);
    let resumed = fourth.iter().find(|event| is_terminal(event)).unwrap();
    assert_eq!(
        resumed["type"],
        "run.completed",
        "{resumed} 事件={:?}",
        types_of(&fourth)
    );
    assert_eq!(resumed["stopReason"], "end_turn");
    assert_eq!(sink.text(&run4), "取消后可以继续。");
    assert!(
        !fourth
            .iter()
            .any(|event| event["type"] == "agent_backend.session"
                || event["type"] == "agent_backend.launching"),
        "取消后应继续同一个会话: {:?}",
        types_of(&fourth)
    );
    assert_eq!(
        launcher.launches.load(Ordering::SeqCst),
        1,
        "第 4 轮不应重新启动后端"
    );
    assert!(
        !process_ended(backend_pid),
        "第 4 轮期间后端应仍是原来的进程: {backend_pid}"
    );
    assert_eq!(
        runtime
            .session_info(&conversation)
            .unwrap()
            .native_session_id
            .as_deref(),
        Some(native_session.as_str())
    );

    let report = tokio::time::timeout(WAIT, runtime.close_conversation(&conversation, "测试结束"))
        .await
        .unwrap_or_else(|_| panic!("关闭会话超过 {} 秒", WAIT.as_secs()));
    let report = report.expect("会话应存在");
    assert_eq!(
        report.confirmation,
        StopConfirmation::Confirmed,
        "关闭应确认进程树已经结束: {:?}",
        report.confirmation
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_acp_full_turn_pi() {
    run_full_turn(AgentBackend::Pi).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_acp_full_turn_codex() {
    run_full_turn(AgentBackend::Codex).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_acp_full_turn_dsh() {
    run_full_turn(AgentBackend::Deepseek).await;
}
