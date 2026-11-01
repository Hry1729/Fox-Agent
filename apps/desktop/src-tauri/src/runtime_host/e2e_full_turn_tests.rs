//! e2e-1101: Fox 原生（PiNative）一整轮的端到端冒烟。
//!
//! 真实 Rust Host + 真实 Node sidecar（`services/agent-runtime/src/pi-runtime.mjs`），
//! 模型是 faux（relay 脚本按 prompt 改写 `modelService.apiType='faux'` 和
//! `fauxResponses`），会话权限是「询问」（ask）。同一个会话依次走完：
//!
//! 1. 写工具出审批卡 → 批准 → 文件写入、结果回写、Run 完成；
//! 2. 同样的写工具 → 拒绝 → 文件不变、结果是错误、Run 仍然完成；
//! 3. 普通多轮：同一个原生会话、同一个 worker；
//! 4. `run_command`（同步）批准后运行长命进程，取消 Run → Run `cancelled`，
//!    子进程按 pid 确认已被回收；
//! 5. 取消后同一会话还能再跑一轮。
//!
//! 和 `native_host_integration_tests` 一样在子进程里跑（测试用 `--exact` 重启自己）。
use super::*;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, PermissionMode, TimeBudgets};
use rusqlite::{params, OptionalExtension};
use std::path::Path;
use std::time::{Duration, Instant};
use tauri::Manager;

const CHILD: &str = "FOX_TEST_E2E_FULL_TURN_CHILD";
const TEST: &str = "runtime_host::e2e_full_turn_tests::e2e_fox_native_full_turn_approval_second_turn_and_cancel";

struct HostCleanup(RuntimeHost);
impl Drop for HostCleanup {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

fn run_status(database: &Database, run: &str) -> String {
    database
        .with_connection(|connection| connection.query_row("SELECT status FROM runs WHERE id=?1", [run], |row| row.get(0)))
        .unwrap()
}

fn event_types(database: &Database, run: &str) -> Vec<String> {
    database
        .with_connection(|connection| {
            let mut statement = connection.prepare("SELECT event_type FROM run_events WHERE run_id=?1 ORDER BY seq")?;
            let rows = statement.query_map([run], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap()
}

fn report(host: &RuntimeHost, database: &Database, run: &str, root: &Path) -> String {
    let diagnostics = host.diagnostics();
    let events = event_types(database, run);
    let tail: Vec<_> = events.iter().rev().take(12).rev().collect();
    format!(
        "run={run} status={} events(tail)={tail:?} evidence={} host_state={} last_error={:?} stderr_tail={:?}",
        run_status(database, run),
        root.display(),
        diagnostics.state,
        diagnostics.last_error,
        diagnostics.stderr_tail
    )
}

fn wait_for(label: &str, seconds: u64, mut done: impl FnMut() -> bool, explain: impl Fn() -> String) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        if Instant::now() >= deadline {
            panic!("timed out ({seconds}s) waiting for {label}: {}", explain());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pending_approval(database: &Database, run: &str) -> Option<String> {
    database
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT a.id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                     WHERE t.run_id=?1 AND a.status='pending' ORDER BY a.requested_at LIMIT 1",
                    [run],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })
        .unwrap()
}

fn approval_status(database: &Database, approval: &str) -> String {
    database
        .with_connection(|connection| connection.query_row("SELECT status FROM approvals WHERE id=?1", [approval], |row| row.get(0)))
        .unwrap()
}

fn native_result(database: &Database, run: &str, call: &str) -> Value {
    let raw: String = database
        .with_connection(|connection| {
            connection.query_row(
                "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",
                params![run, call],
                |row| row.get(0),
            )
        })
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn assistant_text(database: &Database, run: &str) -> String {
    database
        .with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT content FROM messages WHERE run_id=?1 AND role='assistant' ORDER BY ordinal")?;
            let rows = statement.query_map([run], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap()
        .join("\n")
}

fn pi_result_ids(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|entry| entry.get("message").cloned())
        .filter(|message| message["role"] == "toolResult")
        .filter_map(|message| message["toolCallId"].as_str().map(str::to_owned))
        .collect()
}

fn worker_pid(host: &RuntimeHost) -> u32 {
    host.state.lock().unwrap().worker.as_ref().expect("worker").child.lock().unwrap().id()
}

/// Gone = no such process, or (Linux) a zombie nobody will run again.
fn process_gone(pid: u32) -> bool {
    zombie(pid) || super::process_identity::owner_is_gone(Some(i64::from(pid)), None)
}

#[cfg(not(windows))]
fn zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()
        .and_then(|stat| stat.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().chars().next()))
        == Some('Z')
}

#[cfg(windows)]
fn zombie(_pid: u32) -> bool {
    false
}

fn configure_model(database: &Database) {
    database
        .save_model_service(&crate::database::ModelServiceRecord {
            name: "e2e-native-faux".into(),
            base_url: "faux://fox-e2e-full-turn".into(),
            model_id: "fox-test".into(),
            api_type: "faux".into(),
            context_window: 32768,
            max_output_tokens: 4096,
            supports_image_input: false,
            enabled: true,
            credential_configured: false,
            connection_type: "local".into(),
            last_status: "unknown".into(),
            last_latency_ms: None,
            last_checked_at: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
}

/// The long-lived command of turn 4: it writes its own pid into the project
/// and then sleeps. Unix `exec` keeps the pid; on Windows the PowerShell
/// process (a grandchild of cmd.exe) writes its `$PID`.
fn long_command() -> &'static str {
    if cfg!(windows) {
        "powershell -NoProfile -NonInteractive -Command Set-Content -Path pid.txt -Value $PID; Start-Sleep -Seconds 120"
    } else {
        "echo $$ > pid.txt; exec sleep 120"
    }
}

fn write_relay(root: &Path) -> PathBuf {
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../services/agent-runtime/src/pi-runtime.mjs");
    assert!(runtime.is_absolute() && runtime.is_file(), "real Pi runtime source is absent");
    let script = root.join("e2e-runtime-relay.mjs");
    let constants = format!(
        "const runtime = {};\nconst root = {};\nconst longCommand = {};\n",
        crate::canonical_json::to_string(&runtime.to_string_lossy()).unwrap(),
        crate::canonical_json::to_string(&root.to_string_lossy()).unwrap(),
        crate::canonical_json::to_string(&long_command()).unwrap(),
    );
    let relay = r#"
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { appendFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
const nativePath=value=>value.startsWith('\\\\?\\UNC\\')?'\\\\'+value.slice(8):value.startsWith('\\\\?\\')?value.slice(4):value
const entry=nativePath(runtime)
const child=spawn(process.execPath,[entry],{cwd:dirname(entry),windowsHide:true,shell:false,stdio:['pipe','pipe','pipe']})
const log=(direction,frame)=>appendFileSync(join(root,'wire.jsonl'),JSON.stringify({direction,frame})+'\n')
createInterface({input:child.stdout}).on('line',line=>{
  try{log('runtime',JSON.parse(line))}catch{}
  process.stdout.write(line+'\n')
})
child.stderr.on('data',chunk=>{appendFileSync(join(root,'sidecar.stderr.log'),chunk);process.stderr.write(chunk)})
child.once('error',error=>{writeFileSync(join(root,'relay-error.log'),String(error.stack??error));process.exit(1)})
child.once('exit',(code,signal)=>process.exit(code??(signal?1:0)))
createInterface({input:process.stdin}).on('line',line=>{
  const frame=JSON.parse(line)
  if(frame.type==='prompt'){
    const text=frame.payload.text
    const service=frame.payload.modelService
    service.apiType='faux'
    service.fauxTokensPerSecond=10000
    if(text==='e2e-approve')service.fauxResponses=[
      {content:[{type:'toolCall',id:'e2e-write-approved',name:'write_file',arguments:{path:'approved.txt',content:'approved-by-user'}}],stopReason:'toolUse'},
      'e2e approve turn complete']
    else if(text==='e2e-deny')service.fauxResponses=[
      {content:[{type:'toolCall',id:'e2e-write-denied',name:'write_file',arguments:{path:'denied.txt',content:'must-not-exist'}}],stopReason:'toolUse'},
      'e2e deny turn complete']
    else if(text==='e2e-plain')service.fauxResponses=['e2e plain turn complete']
    else if(text==='e2e-reap')service.fauxResponses=[
      {content:[{type:'toolCall',id:'e2e-long-command',name:'run_command',arguments:{command:longCommand,timeoutSeconds:120}}],stopReason:'toolUse'},
      'e2e reap turn must not complete']
    else if(text==='e2e-after-cancel')service.fauxResponses=['e2e after cancel complete']
    else throw new Error('unrecognized e2e prompt: '+text)
  }
  log('host',frame)
  child.stdin.write(JSON.stringify(frame)+'\n')
}).once('close',()=>child.stdin.end())
process.once('exit',()=>{try{child.kill()}catch{}})
"#;
    std::fs::write(&script, format!("{constants}{relay}")).unwrap();
    script
}

#[test]
fn e2e_fox_native_full_turn_approval_second_turn_and_cancel() {
    if std::env::var_os(CHILD).is_none() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env_remove("FOX_RUNTIME_EXECUTABLE")
            .env_remove("FOX_RUNTIME_SCRIPT")
            .env_remove("FOX_RUNTIME_MODE")
            .env("FOX_KERNEL_MODE", "authoritative")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        // Drain both pipes so a chatty child can never block on a full pipe.
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let out = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
            text
        });
        let err = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
            text
        });
        let deadline = Instant::now() + Duration::from_secs(240);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "e2e native full turn exceeded 240 seconds\nchild stdout: {}\nchild stderr: {}",
                    out.join().unwrap_or_default(),
                    err.join().unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
        assert!(status.success(), "child stdout: {stdout}\nchild stderr: {stderr}");
        return;
    }

    let parent = std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root = parent.join(format!("fox-e2e-full-turn-{}", uuid::Uuid::new_v4()));
    println!("e2e native full turn fixture: {}", root.display());
    for directory in [&root, &root.join("sessions"), &root.join("attachments"), &root.join("skills"), &root.join("project")] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::env::set_var("FOX_RUNTIME_SCRIPT", write_relay(&root));
    let project = root.join("project");
    let database = Database::open(root.join("facts.db")).unwrap();
    configure_model(&database);
    let conversation = database
        .create_conversation(database.default_agent_id(), Some("e2e native"), Some(project.to_str().unwrap()), Some("ask"))
        .unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {
        window.create = false;
    }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let client = crate::yuxi::YuxiClient::new().unwrap();
    let host = RuntimeHost::new(
        app.handle().clone(),
        database.clone(),
        root.join("sessions"),
        root.join("attachments"),
        root.join("skills"),
        client.clone(),
    );
    let _cleanup = HostCleanup(host.clone());
    app.manage(crate::app_state::AppState::new(
        database.clone(),
        host.clone(),
        client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(), database.clone(), client),
        crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(
            &crate::local_knowledge_storage::resolve_startup_root(&root, None),
            None,
        ),
        root.clone(),
        root.join("skills"),
    ));

    let start = |text: &str| -> String {
        let started = database.create_run(&conversation.id, text, None).unwrap();
        let binding = database
            .freeze_default_run_control_for_engine(&started.run.id, "legacy", TimeBudgets::continuous(), "pi")
            .unwrap();
        assert_eq!(binding.authority, ExecutionAuthority::Authoritative);
        assert_eq!(binding.executor_route, Some(ExecutorRoute::PiNative));
        assert_eq!(binding.permission.mode, PermissionMode::Ask, "the conversation runs in 询问");
        if let Err(error) = host.start_run(&started, text, &[]) {
            panic!("native start failed: {error}; {}", report(&host, &database, &started.run.id, &root));
        }
        started.run.id
    };
    let settled = |run: &str, status: &str, seconds: u64| {
        wait_for(&format!("Run {status}"), seconds, || run_status(&database, run) == status, || report(&host, &database, run, &root));
        wait_for(
            "Run session ownership released",
            seconds,
            || host.state.lock().unwrap().worker.as_ref().map_or(true, |worker| worker.sessions.by_run(run).is_none()),
            || report(&host, &database, run, &root),
        );
    };
    let await_card = |run: &str| -> String {
        let mut found = None;
        wait_for(
            "approval card",
            60,
            || {
                found = pending_approval(&database, run);
                found.is_some()
            },
            || report(&host, &database, run, &root),
        );
        let approval = found.unwrap();
        assert!(database.native_run_waiting_approval(run).unwrap(), "the Run waits for the user");
        assert_eq!(run_status(&database, run), "running");
        approval
    };

    // ── 1. 写工具出卡 → 批准 ──────────────────────────────────────────────
    let approve = start("e2e-approve");
    let card = await_card(&approve);
    assert!(!project.join("approved.txt").exists(), "nothing is written before the user approves");
    assert!(host.resolve_approval(&card, ApprovalDecision::AllowOnce).unwrap());
    settled(&approve, "completed", 60);
    assert_eq!(approval_status(&database, &card), "approved");
    assert_eq!(std::fs::read_to_string(project.join("approved.txt")).unwrap(), "approved-by-user");
    let written = native_result(&database, &approve, "e2e-write-approved");
    assert_ne!(written["isError"], true, "{written}");
    assert!(assistant_text(&database, &approve).contains("e2e approve turn complete"));
    let events = event_types(&database, &approve);
    assert!(events.iter().any(|kind| kind == "message.delta"), "streamed text: {events:?}");
    let session = database.get_runtime_session(&conversation.id).unwrap().expect("native session");
    assert!(session.is_native());
    let session_file = session.native_path().expect("native session file");
    assert!(pi_result_ids(&session_file).contains(&"e2e-write-approved".to_owned()), "Pi received the tool result");
    let pid = worker_pid(&host);

    // ── 2. 同样的写工具 → 拒绝 ───────────────────────────────────────────
    let deny = start("e2e-deny");
    let card = await_card(&deny);
    assert!(host.resolve_approval(&card, ApprovalDecision::Deny).unwrap());
    settled(&deny, "completed", 60);
    assert_eq!(approval_status(&database, &card), "denied");
    assert!(!project.join("denied.txt").exists(), "a denied write leaves no file");
    let refused = native_result(&database, &deny, "e2e-write-denied");
    assert_eq!(refused["isError"], true, "the model sees the refusal: {refused}");
    assert!(assistant_text(&database, &deny).contains("e2e deny turn complete"));
    assert!(pi_result_ids(&session_file).contains(&"e2e-write-denied".to_owned()));

    // ── 3. 普通多轮：同一个原生会话、同一个 worker ─────────────────────────
    let plain = start("e2e-plain");
    settled(&plain, "completed", 60);
    assert!(assistant_text(&database, &plain).contains("e2e plain turn complete"));
    let same = database.get_runtime_session(&conversation.id).unwrap().unwrap();
    assert_eq!(same.runtime_session_id, session.runtime_session_id);
    assert_eq!(same.native_session_path, session.native_session_path);
    assert_eq!(worker_pid(&host), pid, "one worker serves every turn");

    // ── 4. run_command 运行中取消 → 子进程被回收 ─────────────────────────
    let reap = start("e2e-reap");
    let card = await_card(&reap);
    assert!(host.resolve_approval(&card, ApprovalDecision::AllowOnce).unwrap());
    let pid_file = project.join("pid.txt");
    let mut command_pid = 0u32;
    wait_for(
        "the long command to report its pid",
        60,
        || {
            command_pid = std::fs::read_to_string(&pid_file).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
            command_pid > 0
        },
        || report(&host, &database, &reap, &root),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(!process_gone(command_pid), "the command is running before the cancel (pid {command_pid})");
    assert_eq!(run_status(&database, &reap), "running");
    assert!(host.cancel_run(&reap).unwrap(), "{}", report(&host, &database, &reap, &root));
    settled(&reap, "cancelled", 30);
    wait_for(
        &format!("command process {command_pid} to be reaped"),
        10,
        || process_gone(command_pid),
        || report(&host, &database, &reap, &root),
    );
    assert!(!assistant_text(&database, &reap).contains("must not complete"));

    // ── 5. 取消后同一会话还能继续 ─────────────────────────────────────────
    let after = start("e2e-after-cancel");
    settled(&after, "completed", 60);
    assert!(assistant_text(&database, &after).contains("e2e after cancel complete"));
    let continued = database.get_runtime_session(&conversation.id).unwrap().unwrap();
    assert_eq!(continued.runtime_session_id, session.runtime_session_id);
}
