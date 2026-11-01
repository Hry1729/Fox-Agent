//! 取消被后端确认后，Host 清扫本轮新起的工具子进程，后端进程和会话留下来。
//! 没有 node 时跳过；CI 上没有 node 则失败。
#![cfg(feature = "acp")]

use fox_agent_backend::acp::AcpConnection;
use fox_agent_backend::capability_view::PiLaunch;
use fox_agent_backend::external::{
    ExternalLauncher, ExternalRuntime, ExternalSink, LaunchFailure, LaunchRequest, Launched, TurnRequest,
};
use fox_agent_backend::external_home::{LaunchDeclaration, PermissionMode};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::projection::{PermissionCard, PermissionOption};
use fox_agent_backend::proctree::process_ended;
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Recorded {
    events: Vec<(String, Value)>,
    audits: Vec<(String, String, Value)>,
}

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Recorded>>);

impl ExternalSink for Sink {
    fn emit(&self, run_id: &str, _conversation_id: &str, event: Value) -> Result<(), String> {
        self.0.lock().unwrap().events.push((run_id.to_owned(), event));
        Ok(())
    }

    fn request_approval(&self, _run_id: &str, _conversation_id: &str, _key: &str, _card: &PermissionCard) -> Result<String, String> {
        Ok("appr-unused".into())
    }

    fn audit(&self, conversation_id: &str, _backend: AgentBackend, event: &str, details: &Value) {
        self.0.lock().unwrap().audits.push((conversation_id.to_owned(), event.to_owned(), details.clone()));
    }

    fn host_permission_answer(
        &self,
        _run_id: &str,
        _conversation_id: &str,
        _backend: AgentBackend,
        _native_mode: &str,
        _request: &Value,
        _options: &[PermissionOption],
    ) -> Option<fox_agent_backend::external::HostPermissionAnswer> {
        None
    }
}

impl Sink {
    fn events(&self, run_id: &str) -> Vec<Value> {
        self.0.lock().unwrap().events.iter().filter(|(run, _)| run == run_id).map(|(_, event)| event.clone()).collect()
    }

    fn text(&self, run_id: &str) -> String {
        self.events(run_id)
            .iter()
            .filter(|event| event["type"] == "message.delta")
            .map(|event| event["delta"].as_str().unwrap_or("").to_owned())
            .collect()
    }

    fn audits(&self) -> Vec<(String, String, Value)> {
        self.0.lock().unwrap().audits.clone()
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

    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let launches = Arc::clone(&self.launches);
        Box::pin(async move {
            launches.fetch_add(1, Ordering::SeqCst);
            let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock-acp-child-agent.mjs");
            let spec = LaunchSpec {
                backend: request.backend,
                program: "node".into(),
                args: vec![script.into()],
                version_pin: "mock",
                credential_env: &[],
                experimental: false,
                session_env: vec![],
            };
            let connection = AcpConnection::spawn_detailed(&spec, &request.project_root).await.map_err(LaunchFailure::from)?;
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

fn require_node() -> bool {
    if std::process::Command::new("node").arg("--version").output().is_ok() {
        return true;
    }
    if std::env::var_os("CI").is_some() {
        panic!("CI 上没有 node，取消清扫测试不能跳过");
    }
    eprintln!("SKIPPED: node not installed");
    false
}

fn turn(conversation: &str, run: &str, prompt: &str) -> TurnRequest {
    TurnRequest {
        run_id: run.into(),
        prompt: prompt.into(),
        stored_native_session: None,
        launch: LaunchRequest {
            conversation_id: conversation.into(),
            backend: AgentBackend::Pi,
            project_uuid: None,
            project_root: std::env::temp_dir(),
            session_key: format!("k-{run}"),
            full_access_granted: false,
            unified_login: None,
            native_permission: None,
        },
    }
}

/// 测试失败或提前返回时，把假 Agent 故意留下的子进程杀掉，避免泄漏。
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

#[cfg(target_os = "linux")]
fn parent_pid(pid: u32) -> u32 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_else(|error| panic!("读取 /proc/{pid}/stat 失败: {error}"));
    let rest = stat.rfind(')').map(|index| &stat[index + 1..]).unwrap_or("");
    rest.split_whitespace().nth(1).and_then(|value| value.parse().ok()).unwrap_or(0)
}

/// Windows：`CREATE_NO_WINDOW` 的子进程会带一个无窗口的 conhost.exe（父进程就是该子进程），
/// 它和子进程一起属于本轮新起的进程，也在 Job 里，清扫时应一并结束。
#[cfg(windows)]
fn console_hosts_of(parents: &[u32]) -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    assert!(snapshot != INVALID_HANDLE_VALUE, "CreateToolhelp32Snapshot 失败");
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut hosts = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        let len = entry.szExeFile.iter().position(|unit| *unit == 0).unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
        if name == "conhost.exe" && parents.contains(&entry.th32ParentProcessID) {
            hosts.push(entry.th32ProcessID);
        }
        ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    hosts
}

fn pids_from(text: &str) -> Vec<u32> {
    let rest = text.split("pids:").nth(1).unwrap_or_else(|| panic!("没有 pids 回报: {text}"));
    rest.split(|ch: char| !ch.is_ascii_digit() && ch != ',')
        .next()
        .unwrap_or("")
        .split(',')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or_else(|error| panic!("pid {part}: {error}")))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmed_cancel_sweeps_new_children_and_keeps_the_backend() {
    if !require_node() {
        return;
    }
    let sink = Sink::default();
    let launcher = MockLauncher::default();
    let runtime = ExternalRuntime::with_cancel_timeout(sink.clone(), launcher.clone(), tokio::runtime::Handle::current(), Duration::from_secs(5));
    runtime.start_turn(turn("sweep", "spawn", "spawn-child")).unwrap();
    let mut reported = String::new();
    for _ in 0..200 {
        reported = sink.text("spawn");
        if reported.contains("pids:") && reported.contains(',') {
            break;
        }
        if reported.contains("spawn-failed") {
            panic!("假 Agent 没能拉起子进程: {reported}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let child_pids = pids_from(&reported);
    assert_eq!(child_pids.len(), 2, "应回报两个子进程: {reported}");
    assert!(child_pids.iter().all(|pid| !process_ended(*pid)), "清扫前子进程应仍在: {child_pids:?}");
    let _reap = Reap(child_pids.clone());
    // 本轮新起的进程：两个子进程，Windows 上再加它们各自的 conhost.exe（取消前记下，按映像名和父进程确认）。
    #[allow(unused_mut)]
    let mut turn_pids = child_pids.clone();
    #[cfg(windows)]
    turn_pids.extend(console_hosts_of(&child_pids));
    #[cfg(target_os = "linux")]
    let backend_pid = parent_pid(child_pids[0]);

    assert!(matches!(runtime.cancel("spawn"), fox_agent_backend::external::CancelOutcome::Requested { .. }));
    let terminal = wait_terminal(&sink, "spawn").await;
    assert_eq!(terminal["type"], "run.cancelled", "{terminal}");
    assert!(terminal.get("outcome").is_none(), "后端确认了取消: {terminal}");

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if child_pids.iter().all(|pid| process_ended(*pid)) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("宽限期之后子进程仍在: {:?}", child_pids.iter().map(|pid| (*pid, process_ended(*pid))).collect::<Vec<_>>());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let swept = wait_swept(&sink, "spawn").await;
    let killed = swept["killed"]["pids"].as_array().unwrap_or_else(|| panic!("killed.pids 缺失: {swept}"));
    let mut killed_pids: Vec<u32> = killed.iter().filter_map(|value| value.as_u64().map(|pid| pid as u32)).collect();
    killed_pids.sort_unstable();
    let mut expected = turn_pids.clone();
    expected.sort_unstable();
    assert_eq!(killed_pids, expected, "killed 应正好是这两个子进程（Windows 上加它们的 conhost.exe）: {swept}");
    assert_eq!(swept["killed"]["count"], expected.len(), "{swept}");
    assert_eq!(swept["noBaseline"], false, "{swept}");
    assert!(swept["failed"].as_array().is_some_and(|failed| failed.is_empty()), "{swept}");
    let audits = sink.audits();
    assert!(
        audits.iter().any(|(_, event, details)| {
            event == "agent_backend.cancel_swept" && child_pids.iter().all(|pid| details["killed"]["pids"].as_array().is_some_and(|pids| pids.iter().any(|value| value.as_u64() == Some(u64::from(*pid)))))
        }),
        "审计里应有 cancel_swept: {audits:?}"
    );

    assert!(runtime.session_info("sweep").unwrap().live, "后端连接应仍在");
    #[cfg(target_os = "linux")]
    assert!(!process_ended(backend_pid), "后端进程本身应仍在: {backend_pid}");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);

    runtime.start_turn(turn("sweep", "hello", "hello")).unwrap();
    let hello = wait_terminal(&sink, "hello").await;
    assert_eq!(hello["type"], "run.completed", "{hello} text={}", sink.text("hello"));
    assert_eq!(sink.text("hello"), "hello-ok");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1, "取消后下一轮不应重启后端");
    assert!(runtime.session_info("sweep").unwrap().live);
    runtime.shutdown().await;
}

async fn wait_terminal(sink: &Sink, run_id: &str) -> Value {
    for _ in 0..200 {
        if let Some(event) = sink.events(run_id).into_iter().find(|event| {
            event["type"].as_str().is_some_and(|kind| matches!(kind, "run.completed" | "run.cancelled" | "run.failed" | "run.interrupted"))
        }) {
            return event;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("没有等到 {run_id} 结束: {:?}", sink.events(run_id));
}

async fn wait_swept(sink: &Sink, run_id: &str) -> Value {
    for _ in 0..100 {
        if let Some(event) = sink.events(run_id).into_iter().find(|event| event["type"] == "agent_backend.cancel_swept") {
            return event;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("没有等到 agent_backend.cancel_swept: {:?}", sink.events(run_id));
}

/// `pid` 的直接子进程（Windows：Toolhelp；Linux：/proc）。
fn children_of(pid: u32) -> Vec<u32> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        assert!(snapshot != INVALID_HANDLE_VALUE, "CreateToolhelp32Snapshot 失败");
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Vec::new();
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while ok {
            if entry.th32ParentProcessID == pid {
                found.push(entry.th32ProcessID);
            }
            ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        unsafe { CloseHandle(snapshot) };
        found
    }
    #[cfg(target_os = "linux")]
    {
        let mut found = Vec::new();
        for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(candidate) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
            if !process_ended(candidate) && parent_pid_or_zero(candidate) == pid {
                found.push(candidate);
            }
        }
        found
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = pid;
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn parent_pid_or_zero(pid: u32) -> u32 {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return 0 };
    let rest = stat.rfind(')').map(|index| &stat[index + 1..]).unwrap_or("");
    rest.split_whitespace().nth(1).and_then(|value| value.parse().ok()).unwrap_or(0)
}

/// 13.35 P1：外部后端本轮起的 shell 再起的孙进程（Codex 实测是 pwsh -> PING.EXE），
/// 取消后 3 秒内全部结束；`agent_backend.cancel_swept` 在 `run.cancelled` 之前发出
/// （终态之后 Host 拒收 Run 事件），并且记下这些进程。
#[tokio::test(flavor = "multi_thread")]
async fn confirmed_cancel_sweeps_grandchildren_within_three_seconds_before_the_terminal() {
    if !require_node() {
        return;
    }
    let sink = Sink::default();
    let launcher = MockLauncher::default();
    let runtime = ExternalRuntime::with_cancel_timeout(sink.clone(), launcher.clone(), tokio::runtime::Handle::current(), Duration::from_secs(5));
    runtime.start_turn(turn("grand", "grand", "spawn-grandchild")).unwrap();
    let mut shell = 0u32;
    for _ in 0..200 {
        let reported = sink.text("grand");
        if let Some(rest) = reported.split("shell:").nth(1) {
            shell = rest.split(';').next().unwrap_or("").parse().unwrap_or_else(|error| panic!("shell pid: {error} in {reported}"));
            break;
        }
        if reported.contains("spawn-failed") {
            panic!("假 Agent 没能拉起 shell: {reported}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(shell != 0, "没有回报 shell pid: {}", sink.text("grand"));
    // 等孙进程出现（ping / sleep；Windows 上 shell 还带一个 conhost.exe）。
    let mut descendants = Vec::new();
    for _ in 0..100 {
        descendants = children_of(shell);
        #[cfg(windows)]
        descendants.retain(|pid| !console_hosts_of(&[shell]).contains(pid));
        if !descendants.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!descendants.is_empty(), "shell {shell} 没有起孙进程");
    let mut all = vec![shell];
    all.extend(descendants.iter().copied());
    let _reap = Reap(all.clone());
    assert!(all.iter().all(|pid| !process_ended(*pid)), "取消前都应在: {all:?}");

    let cancelled_at = std::time::Instant::now();
    assert!(matches!(runtime.cancel("grand"), fox_agent_backend::external::CancelOutcome::Requested { .. }));
    loop {
        if all.iter().all(|pid| process_ended(*pid)) {
            break;
        }
        assert!(
            cancelled_at.elapsed() < Duration::from_secs(3),
            "取消 3 秒后仍有进程: {:?}",
            all.iter().map(|pid| (*pid, process_ended(*pid))).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let terminal = wait_terminal(&sink, "grand").await;
    assert_eq!(terminal["type"], "run.cancelled", "{terminal}");
    let events = sink.events("grand");
    let swept_at = events.iter().position(|event| event["type"] == "agent_backend.cancel_swept").unwrap_or_else(|| panic!("没有 cancel_swept: {events:?}"));
    let terminal_at = events.iter().position(|event| event["type"] == "run.cancelled").unwrap();
    assert!(swept_at < terminal_at, "cancel_swept 必须在 run.cancelled 之前: {events:?}");
    let swept = &events[swept_at];
    let killed: Vec<u32> = swept["killed"]["pids"].as_array().unwrap_or_else(|| panic!("killed.pids 缺失: {swept}"))
        .iter().filter_map(|value| value.as_u64().map(|pid| pid as u32)).collect();
    for pid in &all {
        assert!(killed.contains(pid), "pid {pid} 应在 killed 里: {swept}");
    }
    assert!(swept["failed"].as_array().is_some_and(|failed| failed.is_empty()), "{swept}");
    assert!(sink.audits().iter().any(|(_, event, _)| event == "agent_backend.cancel_swept"), "审计里应有 cancel_swept");
    assert!(runtime.session_info("grand").unwrap().live, "后端连接应仍在");
    runtime.shutdown().await;
}
