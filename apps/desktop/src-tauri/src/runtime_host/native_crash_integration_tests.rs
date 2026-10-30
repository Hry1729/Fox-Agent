//! U06 process-crash fixture. Checkpoints only park the explicitly owned test
//! child; they never manufacture execution receipts or change database facts.

use super::*;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, TimeBudgets};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::time::{Duration, Instant};
use tauri::Manager;

#[cfg(windows)]
#[path = "kernel_model_worker_job.rs"]
mod owned_job;

const TEST: &str = "runtime_host::native_crash_integration_tests::native_host_crash_boundaries_reconcile_without_reexecution";
const CHILD: &str = "FOX_NATIVE_CRASH_CHILD";
const ROOT: &str = "FOX_NATIVE_CRASH_ROOT";
const NONCE: &str = "FOX_NATIVE_CRASH_NONCE";
const STAGE: &str = "FOX_NATIVE_CRASH_STAGE";
const PHASE: &str = "FOX_NATIVE_CRASH_PHASE";
const CALL: &str = "native-crash-edit";
const BEFORE: &str = "before-native-crash";
const AFTER: &str = "after-native-crash";
const STAGES: [&str; 7] = ["register_before_ack", "register_after_ack", "native_claimed",
    "resource_returned", "host_result_committed", "native_result_before_delivery", "pi_result_persisted"];

fn native_path(path: &Path) -> PathBuf {
    let text = path.to_str().expect("isolated crash paths must be Unicode");
    if let Some(tail) = text.strip_prefix(r"\\?\UNC\") { PathBuf::from(format!(r"\\{tail}")) }
    else if let Some(tail) = text.strip_prefix(r"\\?\") { PathBuf::from(tail) }
    else { path.to_owned() }
}
fn repository() -> PathBuf {
    native_path(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap())
}
fn read_json(path: &Path) -> Option<Value> { serde_json::from_slice(&std::fs::read(path).ok()?).ok() }
fn write_json(path: &Path, value: &Value) {
    std::fs::write(path,crate::canonical_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn hash(bytes: &[u8]) -> String { format!("sha256:{}",hex::encode(Sha256::digest(bytes))) }

fn owned_scope() -> Option<(PathBuf, Value)> {
    if std::env::var(CHILD).ok().as_deref() != Some("1")
        || std::env::var(PHASE).ok().as_deref() != Some("crash") { return None; }
    let root = native_path(&PathBuf::from(std::env::var_os(ROOT)?));
    if !root.is_absolute() || !root.file_name()?.to_str()?.starts_with("native-crash-") { return None; }
    let resolved = native_path(&root.canonicalize().ok()?);
    let allowed = native_path(&repository().join("migration-work").canonicalize().ok()?);
    if !resolved.starts_with(&allowed) { return None; }
    let owner = read_json(&root.join("owner.json"))?;
    let nonce = std::env::var(NONCE).ok()?;
    if owner["nonce"].as_str() != Some(nonce.as_str())
        || owner["root"].as_str() != resolved.to_str()
        || owner["hostPid"].as_u64() != Some(std::process::id() as u64) { return None; }
    Some((root,owner))
}

pub(super) fn checkpoint(stage: &str, run_id: &str, tool_call_id: &str) {
    if !["native_claimed","resource_returned","host_result_committed"].contains(&stage)
        || std::env::var(STAGE).ok().as_deref() != Some(stage) { return; }
    let Some((root,owner)) = owned_scope() else { return; };
    let Some(intent) = read_json(&root.join("intent.json")) else { return; };
    if intent["nonce"] != owner["nonce"] || intent["stage"].as_str() != Some(stage)
        || intent["runId"].as_str() != Some(run_id) || intent["callId"].as_str() != Some(tool_call_id) { return; }
    let witness = json!({"stage":stage,"runId":run_id,"callId":tool_call_id,"hostPid":std::process::id(),
        "nonce":owner["nonce"],"origin":"Rust checkpoint after the real preceding boundary","at":crate::database::now_ms()});
    let Ok(mut file) = std::fs::OpenOptions::new().write(true).create_new(true).open(root.join("boundary.json")) else { return; };
    if file.write_all(&crate::canonical_json::to_vec(&witness).unwrap()).and_then(|_|file.sync_all()).is_err() { std::process::exit(74); }
    let deadline = Instant::now()+Duration::from_secs(30);
    while Instant::now()<deadline { thread::sleep(Duration::from_millis(20)); }
    let _ = std::fs::write(root.join("checkpoint-timeout.txt"),stage);
    std::process::exit(74);
}

fn relay(root: &Path, phase: &str, stage: &str, nonce: &str) -> PathBuf {
    let runtime = repository().join("services/agent-runtime/src/pi-runtime.mjs");
    let binary = std::env::var_os("FOX_TEST_NATIVE_SIDECAR_BIN").map(|value| {
        let path = native_path(&PathBuf::from(value).canonicalize().unwrap());
        assert!(path.is_file()); path
    });
    let config = json!({"root":root,"runtime":runtime,"binary":binary,"phase":phase,"stage":stage,
        "nonce":nonce,"hostPid":std::process::id(),"call":CALL,"observeCall":"native-crash-observe","before":BEFORE,"after":AFTER});
    let script = root.join(format!("{phase}-relay.mjs"));
    let source = r#"
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { appendFileSync, readFileSync, writeFileSync, openSync, writeSync, fsyncSync, closeSync } from 'node:fs'
import { dirname, join } from 'node:path'
const nativePath=value=>value.startsWith('\\\\?\\UNC\\')?'\\\\'+value.slice(8):value.startsWith('\\\\?\\')?value.slice(4):value
const executable=cfg.binary?nativePath(cfg.binary):process.execPath
const args=cfg.binary?[]:[nativePath(cfg.runtime)]
const child=spawn(executable,args,{cwd:dirname(cfg.binary?executable:args[0]),windowsHide:true,shell:false,stdio:['pipe','pipe','pipe']})
writeFileSync(join(cfg.root,`${cfg.phase}-sidecar.json`),JSON.stringify({nonce:cfg.nonce,hostPid:cfg.hostPid,relayPid:process.pid,piPid:child.pid,executable,args}))
const log=(direction,frame,forwarded)=>appendFileSync(join(cfg.root,`${cfg.phase}-wire.jsonl`),JSON.stringify({direction,forwarded,frame})+'\n')
let registerId,executeId,targetResponseDelivered=false
const intent=()=>JSON.parse(readFileSync(join(cfg.root,'intent.json'),'utf8'))
const hit=(stage,frame)=>{
  if(cfg.phase!=='crash'||cfg.stage!==stage)return false
  const owner=JSON.parse(readFileSync(join(cfg.root,'owner.json'),'utf8'))
  const target=intent()
  if(owner.nonce!==cfg.nonce||owner.hostPid!==cfg.hostPid||target.nonce!==cfg.nonce||target.stage!==stage||target.callId!==cfg.call||frame.runId!==target.runId)return false
  writeFileSync(join(cfg.root,'held-frame.json'),JSON.stringify(frame))
  const fd=openSync(join(cfg.root,'boundary.json'),'wx')
  writeSync(fd,JSON.stringify({stage,runId:target.runId,callId:target.callId,hostPid:cfg.hostPid,nonce:cfg.nonce,origin:'relay held a real protocol frame',frameId:frame.id}))
  fsyncSync(fd);closeSync(fd)
  return true
}
createInterface({input:child.stdout}).on('line',line=>{
  const frame=JSON.parse(line)
  if(frame.kind==='request'&&frame.type==='tool_batch.register'&&frame.payload.calls.some(call=>call.callId===cfg.call))registerId=frame.id
  if(frame.kind==='request'&&frame.type==='tool_call.execute'&&frame.payload.callId===cfg.call){
    executeId=frame.id
    if(hit('register_after_ack',frame)){log('pi',frame,false);return}
  }
  if(frame.kind==='request'&&frame.type==='model_request.admit'&&targetResponseDelivered){
    if(hit('pi_result_persisted',frame)){log('pi',frame,false);return}
  }
  log('pi',frame,true);process.stdout.write(line+'\n')
})
createInterface({input:process.stdin}).on('line',line=>{
  const frame=JSON.parse(line)
  if(frame.type==='prompt'){
    frame.payload.modelService.apiType='faux'
    frame.payload.modelService.fauxTokensPerSecond=10000
    frame.payload.modelService.fauxResponses=cfg.phase==='crash'?[
      {content:[{type:'toolCall',id:cfg.observeCall,name:'read',arguments:{path:'note.txt'}}],stopReason:'toolUse'},
      {content:[{type:'toolCall',id:cfg.call,name:'edit_file',arguments:{path:'note.txt',oldText:cfg.before,newText:cfg.after}}],stopReason:'toolUse'},
      '原调用已结算。']:['恢复完成；不重新执行原工具。']
  }
  if(frame.requestId===registerId&&frame.type==='tool_batch.registered'&&hit('register_before_ack',frame)){log('host',frame,false);return}
  if(frame.requestId===executeId&&frame.type.startsWith('tool.execute_')&&hit('native_result_before_delivery',frame)){log('host',frame,false);return}
  if(frame.requestId===executeId&&frame.type.startsWith('tool.execute_'))targetResponseDelivered=true
  log('host',frame,true);child.stdin.write(JSON.stringify(frame)+'\n')
}).once('close',()=>child.stdin.end())
child.stderr.on('data',chunk=>{appendFileSync(join(cfg.root,`${cfg.phase}-sidecar.stderr.log`),chunk);process.stderr.write(chunk)})
child.once('error',error=>{process.stderr.write(String(error.stack??error));process.exit(1)})
child.once('exit',(code,signal)=>process.exit(code??(signal?1:0)))
process.once('exit',()=>{try{child.kill()}catch{}})
"#;
    std::fs::write(&script,format!("const cfg={};\n{source}",config.canonical_string())).unwrap();
    script
}

fn branch(entries: &[Value]) -> Vec<Value> {
    let by_id: HashMap<_,_> = entries.iter().filter_map(|entry|Some((entry["id"].as_str()?,entry))).collect();
    let mut current = entries.iter().rev().find(|entry|entry.get("parentId").is_some());
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    while let Some(entry) = current {
        let Some(id) = entry["id"].as_str() else { break };
        assert!(visited.insert(id),"Pi JSONL contains a parent cycle");
        result.push(entry.clone());
        current = entry["parentId"].as_str().and_then(|parent|by_id.get(parent).copied());
    }
    result.reverse(); result
}

fn snapshot(root: &Path, intent: &Value) -> Result<Value,String> {
    let connection = Connection::open_with_flags(root.join("facts.db"),OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|error|error.to_string())?;
    connection.busy_timeout(Duration::from_secs(2)).map_err(|error|error.to_string())?;
    let run = intent["runId"].as_str().ok_or("intent has no Run")?;
    let call = intent["callId"].as_str().ok_or("intent has no call")?;
    let conversation = intent["conversationId"].as_str().ok_or("intent has no conversation")?;
    let ledger = connection.query_row(
        "SELECT state,claimed_at,result_json,canonical_input_json,input_digest,batch_id,context_generation FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",
        params![run,call],|row|Ok(json!({"state":row.get::<_,String>(0)?,"claimedAt":row.get::<_,Option<i64>>(1)?,
            "responseJson":row.get::<_,Option<String>>(2)?,"inputJson":row.get::<_,String>(3)?,
            "inputDigest":row.get::<_,String>(4)?,"batchId":row.get::<_,String>(5)?,"generation":row.get::<_,i64>(6)?})))
        .optional().map_err(|error|error.to_string())?;
    let tool = connection.query_row("SELECT status,result_json,error_message,input_json FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2",
        params![run,call],|row|Ok(json!({"status":row.get::<_,String>(0)?,"resultJson":row.get::<_,Option<String>>(1)?,
            "error":row.get::<_,Option<String>>(2)?,"inputJson":row.get::<_,String>(3)?}))).optional().map_err(|error|error.to_string())?;
    let session = connection.query_row("SELECT runtime_session_id,native_session_path,context_generation,status FROM runtime_sessions WHERE conversation_id=?1 AND runtime_type='pi'",
        [conversation],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,i64>(2)?,row.get::<_,String>(3)?)))
        .optional().map_err(|error|error.to_string())?;
    let (session_id,path,generation,session_status) = session.ok_or("native session mapping is absent")?;
    let path = path.ok_or("native session path is absent")?;
    let bytes = std::fs::read(&path).map_err(|error|error.to_string())?;
    let entries: Vec<Value> = String::from_utf8(bytes.clone()).map_err(|error|error.to_string())?.lines()
        .map(serde_json::from_str).collect::<Result<_,_>>().map_err(|error|error.to_string())?;
    let active = branch(&entries);
    let calls: Vec<_> = active.iter().filter(|entry|entry["message"]["role"] == "assistant")
        .flat_map(|entry|entry["message"]["content"].as_array().into_iter().flatten())
        .filter(|block|block["type"] == "toolCall" && block["id"] == call).cloned().collect();
    let results: Vec<_> = active.iter().filter(|entry|entry["message"]["role"] == "toolResult" && entry["message"]["toolCallId"] == call)
        .map(|entry|entry["message"].clone()).collect();
    Ok(json!({"ledger":ledger,"hostTool":tool,"sessionId":session_id,"generation":generation,"sessionStatus":session_status,
        "piPath":path,"piSha256":hash(&bytes),"piEntries":entries,"activeBranch":active,"targetCalls":calls,"targetResults":results,
        "file":std::fs::read_to_string(root.join("project/note.txt")).map_err(|error|error.to_string())?}))
}

fn state_matches(stage: &str, state: &Value) -> bool {
    let ledger = &state["ledger"];
    let calls = state["targetCalls"].as_array().map_or(0,Vec::len);
    let results = state["targetResults"].as_array().map_or(0,Vec::len);
    if calls != 1 { return false; }
    match stage {
        "register_before_ack" | "register_after_ack" => ledger["state"] == "pending" && ledger["claimedAt"].is_null()
            && ledger["responseJson"].is_null() && results == 0 && state["file"] == BEFORE,
        "native_claimed" => ledger["state"] == "running" && ledger["claimedAt"].is_number()
            && ledger["responseJson"].is_null() && results == 0 && state["file"] == BEFORE,
        "resource_returned" => ledger["responseJson"].is_null() && ledger["claimedAt"].is_number()
            && ledger["state"] == "running" && state["hostTool"]["status"] == "running"
            && state["hostTool"]["resultJson"].is_null() && results == 0 && state["file"] == AFTER,
        "host_result_committed" => ledger["responseJson"].is_null() && state["hostTool"]["resultJson"].is_string()
            && state["hostTool"]["status"] == "completed" && results == 0 && state["file"] == AFTER,
        "native_result_before_delivery" => ledger["state"] == "completed" && ledger["responseJson"].is_string()
            && state["hostTool"]["status"] == "completed" && results == 0 && state["file"] == AFTER,
        "pi_result_persisted" => ledger["state"] == "completed" && ledger["responseJson"].is_string()
            && state["hostTool"]["status"] == "completed" && results == 1 && state["file"] == AFTER,
        _ => false,
    }
}

fn wire(root: &Path, phase: &str) -> Vec<Value> {
    std::fs::read_to_string(root.join(format!("{phase}-wire.jsonl"))).unwrap_or_default().lines()
        .filter_map(|line|serde_json::from_str(line).ok()).collect()
}

fn run_child() {
    let root = native_path(&PathBuf::from(std::env::var_os(ROOT).unwrap()));
    let phase = std::env::var(PHASE).unwrap(); let stage = std::env::var(STAGE).unwrap(); let nonce = std::env::var(NONCE).unwrap();
    let deadline = Instant::now()+Duration::from_secs(5);
    loop {
        if read_json(&root.join("owner.json")).is_some_and(|owner|owner["hostPid"].as_u64() == Some(std::process::id() as u64) && owner["nonce"] == nonce) { break; }
        assert!(Instant::now()<deadline,"parent did not establish ownership before Host startup");
        thread::sleep(Duration::from_millis(5));
    }
    for directory in ["sessions","attachments","skills","project"] { std::fs::create_dir_all(root.join(directory)).unwrap(); }
    let database = Database::open(root.join("facts.db")).unwrap();
    if phase == "crash" {
        std::fs::write(root.join("project/note.txt"),BEFORE).unwrap();
        database.save_model_service(&crate::database::ModelServiceRecord {
            name:"native-crash-faux".into(),base_url:"faux://native-crash-integration".into(),model_id:"fox-crash-test".into(),api_type:"faux".into(),
            context_window:32768,max_output_tokens:4096,supports_image_input:false,enabled:true,credential_configured:false,
            connection_type:"local".into(),last_status:"unknown".into(),last_latency_ms:None,last_checked_at:None,created_at:1,updated_at:1,
        }).unwrap();
    } else { database.repair_interrupted_runs().unwrap(); }
    std::env::set_var("FOX_RUNTIME_SCRIPT",relay(&root,&phase,&stage,&nonce));
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create=false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let client = crate::yuxi::YuxiClient::new().unwrap();
    let host = RuntimeHost::new(app.handle().clone(),database.clone(),root.join("sessions"),root.join("attachments"),root.join("skills"),client.clone());
    app.manage(crate::app_state::AppState::new(database.clone(),host.clone(),client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(),database.clone(),client),
        crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(&crate::local_knowledge_storage::resolve_startup_root(&root,None),None),root.clone(),root.join("skills")));
    if phase == "crash" {
        assert!(owned_scope().is_some());
        let conversation = database.create_conversation(database.default_agent_id(),Some("isolated native crash"),root.join("project").to_str(),Some("allow")).unwrap();
        let text = "[NATIVE_CRASH_ORIGINAL] 修改 note.txt 并保存到原文件。读取后把 before-native-crash 更新为 after-native-crash，写回 note.txt。";
        let started = database.create_run(&conversation.id,text,None).unwrap();
        let binding = database.freeze_default_run_control_for_engine(&started.run.id,"legacy",TimeBudgets::continuous(),"pi").unwrap();
        assert_eq!(binding.authority,ExecutionAuthority::Authoritative); assert_eq!(binding.executor_route,Some(ExecutorRoute::PiNative));
        write_json(&root.join("intent.json"),&json!({"nonce":nonce,"stage":stage,"runId":started.run.id,"callId":CALL,"conversationId":conversation.id}));
        if let Err(error) = host.start_run(&started,text,&[]) {
            write_json(&root.join("start-failure.json"),&json!({"error":error,"diagnostics":host.diagnostics()}));
            panic!("original Host failed before crash witness: {error}");
        }
        thread::sleep(Duration::from_secs(60));
        panic!("parent did not crash its owned Host after the boundary");
    }
    host.recover_kernel_runs_detached().unwrap();
    host.recover_native_approvals_detached().unwrap();
    let intent = read_json(&root.join("intent.json")).unwrap();
    let conversation = intent["conversationId"].as_str().unwrap();
    let text = "[NATIVE_CRASH_RESTORE] 继续读取已核对的结果，不重做原操作。";
    let started = database.create_run(conversation,text,None).unwrap();
    database.freeze_default_run_control_for_engine(&started.run.id,"legacy",TimeBudgets::continuous(),"pi").unwrap();
    let result = host.start_run(&started,text,&[]);
    if result.is_ok() {
        let deadline = Instant::now()+Duration::from_secs(30);
        loop {
            let status = database.with_connection(|connection|connection.query_row("SELECT status FROM runs WHERE id=?1",[&started.run.id],|row|row.get::<_,String>(0))).unwrap();
            let released = host.state.lock().unwrap().worker.as_ref().is_none_or(|worker|worker.sessions.by_run(&started.run.id).is_none());
            if run_status_is_terminal(&status) && released { break; }
            assert!(Instant::now()<deadline,"restored Run did not settle and release");
            thread::sleep(Duration::from_millis(5));
        }
    }
    let state = snapshot(&root,&intent).unwrap();
    let status = database.with_connection(|connection|connection.query_row("SELECT status FROM runs WHERE id=?1",[&started.run.id],|row|row.get::<_,String>(0))).unwrap();
    let diagnostics = host.diagnostics();
    let shutdown = host.shutdown();
    write_json(&root.join("restore.json"),&json!({"startError":result.err(),"newRunId":started.run.id,"newRunStatus":status,
        "snapshot":state,"diagnostics":diagnostics,"shutdownError":shutdown.err()}));
}

#[cfg(windows)]
struct OwnedChild {
    child: Child,
    job: Option<owned_job::WorkerJob>,
}
#[cfg(windows)]
impl OwnedChild {
    fn spawn(root: &Path, phase: &str, stage: &str, nonce: &str) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([TEST,"--ignored","--exact","--nocapture","--test-threads=1"])
            .env(CHILD,"1").env(ROOT,root).env(PHASE,phase).env(STAGE,stage).env(NONCE,nonce)
            .env("FOX_DATA_DIR",root).env("FOX_KERNEL_MODE","authoritative").env("FOX_KERNEL_ENGINE","pi")
            .env_remove("FOX_RUNTIME_EXECUTABLE").env_remove("FOX_RUNTIME_SCRIPT").env_remove("FOX_RUNTIME_MODE")
            .env_remove("FOX_KERNEL_WORKER_BINARY").env_remove("FOX_KERNEL_CODEX_BINARY")
            .stdin(Stdio::null()).stdout(std::fs::File::create(root.join(format!("{phase}.stdout.log"))).unwrap())
            .stderr(std::fs::File::create(root.join(format!("{phase}.stderr.log"))).unwrap()).creation_flags(0x0800_0000);
        let child = command.spawn().unwrap();
        let job = owned_job::WorkerJob::attach(&child).expect("bound only the newly spawned Host process tree");
        write_json(&root.join("owner.json"),&json!({"root":root,"nonce":nonce,"hostPid":child.id(),"ownerPid":std::process::id(),"phase":phase}));
        Self { child,job:Some(job) }
    }
    fn crash(&mut self) {
        drop(self.job.take());
        self.child.wait().unwrap();
    }
    fn finish(&mut self) -> bool {
        let deadline = Instant::now()+Duration::from_secs(45);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() { return status.success(); }
            if Instant::now()>=deadline { self.crash(); return false; }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn restored_matches(stage: &str, before: &Value, restored: &Value, records: &[Value]) -> bool {
    let after = &restored["snapshot"];
    let blocked = matches!(stage,"native_claimed" | "resource_returned");
    if before["sessionId"] != after["sessionId"] || before["generation"] != after["generation"] || before["file"] != after["file"] { return false; }
    if records.iter().any(|row|row["direction"] == "pi" && row["frame"]["type"] == "tool_call.execute") { return false; }
    let facts: Vec<_> = records.iter().filter(|row|row["direction"] == "host" && row["frame"]["type"] == "tool_call.facts_completed")
        .flat_map(|row|row["frame"]["payload"]["facts"].as_array().into_iter().flatten()).filter(|fact|fact["callId"] == CALL).collect();
    if facts.is_empty() { return false; }
    if blocked {
        return restored["startError"].is_string() && before["piSha256"] == after["piSha256"]
            && after["targetResults"].as_array().is_some_and(Vec::is_empty)
            && facts.iter().all(|fact|fact["state"] == "outcome_unknown" && fact.get("response").is_none())
            && !records.iter().any(|row|row["direction"] == "host" && row["frame"]["type"] == "prompt");
    }
    if restored["startError"].is_string() || restored["newRunStatus"] != "completed" || restored["shutdownError"].is_string() { return false; }
    let expected = if matches!(stage,"register_before_ack" | "register_after_ack") { "not_executed" } else { "committed" };
    let results = after["targetResults"].as_array().unwrap();
    if results.len()!=1 || !facts.iter().all(|fact|fact["state"] == expected) { return false; }
    let Some(raw) = after["ledger"]["responseJson"].as_str() else { return false; };
    let Ok(body) = serde_json::from_str::<Value>(raw) else { return false; };
    let expected_digest = hash(crate::canonical_json::to_string(&body).unwrap().as_bytes());
    results[0]["details"]["foxNativeHostResponseDigest"] == expected_digest
        && (results[0]["isError"] == true) == (body["isError"] == true)
        && after["ledger"]["inputDigest"] == before["ledger"]["inputDigest"]
        && after["ledger"]["claimedAt"] == before["ledger"]["claimedAt"]
}

#[test]
#[ignore = "U06 isolated Windows Job Object crashes of the real Rust Host and Pi; no user data or remote model"]
fn native_host_crash_boundaries_reconcile_without_reexecution() {
    if std::env::var_os(CHILD).is_some() { run_child(); return; }
    assert!(cfg!(windows),"this fixture requires the existing Windows owned-process Job Object");
    #[cfg(windows)] {
        let stages: Vec<_> = std::env::var("FOX_NATIVE_CRASH_STAGES").map(|value|value.split(',').map(str::trim).map(str::to_owned).collect())
            .unwrap_or_else(|_|STAGES.iter().map(|stage|(*stage).to_owned()).collect());
        assert!(!stages.is_empty() && stages.iter().all(|stage|STAGES.contains(&stage.as_str())));
        let mut outcomes = Vec::new();
        let suite = repository().join("migration-work").join(format!("native-crash-suite-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&suite).unwrap();
        println!("native crash evidence: {}",suite.display());
        for stage in stages {
            let root = repository().join("migration-work").join(format!("native-crash-{}-{}",stage,uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let nonce = uuid::Uuid::new_v4().to_string();
            let mut child = OwnedChild::spawn(&root,"crash",&stage,&nonce);
            let deadline = Instant::now()+Duration::from_secs(45);
            let mut witnessed = None;
            while Instant::now()<deadline {
                if child.child.try_wait().unwrap().is_some() { break; }
                if let (Some(boundary),Some(intent)) = (read_json(&root.join("boundary.json")),read_json(&root.join("intent.json"))) {
                    if boundary["nonce"] == nonce && boundary["hostPid"].as_u64() == Some(child.child.id() as u64) {
                        if let Ok(state) = snapshot(&root,&intent) {
                            if state_matches(&stage,&state) { witnessed=Some((boundary,intent,state)); break; }
                        }
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }
            write_json(&root.join("owned-processes.json"),&json!({"hostPid":child.child.id(),"sidecar":read_json(&root.join("crash-sidecar.json")),
                "crashMethod":"close this test child's Windows KILL_ON_JOB_CLOSE Job Object; no graceful Host shutdown"}));
            child.crash();
            let Some((boundary,intent,before)) = witnessed else {
                outcomes.push(json!({"stage":stage,"valid":false,"reason":"no verified real boundary before crash","root":root})); continue;
            };
            let crashed = snapshot(&root,&intent).unwrap();
            write_json(&root.join("before-crash.json"),&json!({"boundary":boundary,"snapshot":before}));
            write_json(&root.join("after-crash.json"),&crashed);
            let mut recovery = OwnedChild::spawn(&root,"restore",&stage,&nonce);
            let exited = recovery.finish();
            let restored = read_json(&root.join("restore.json")).unwrap_or_else(||json!({}));
            let records = wire(&root,"restore");
            let held_response_matches = stage != "native_result_before_delivery" || read_json(&root.join("held-frame.json"))
                .and_then(|frame|Some(frame["payload"].clone() == serde_json::from_str::<Value>(crashed["ledger"]["responseJson"].as_str()?).ok()?)) == Some(true);
            let valid = held_response_matches && exited && restored_matches(&stage,&crashed,&restored,&records);
            let result = json!({"stage":stage,"valid":valid,"root":root,"restoreChildSucceeded":exited,
                "beforeCrash":before,"afterCrash":crashed,"restore":restored,"heldResponseEqualsCommittedNativePayload":held_response_matches,
                "expected":if matches!(stage.as_str(),"native_claimed" | "resource_returned") {"block_unknown_without_reexecution"} else {"reconcile_without_reexecution"}});
            write_json(&root.join("case.json"),&result); outcomes.push(result);
        }
        let valid = outcomes.iter().all(|case|case["valid"] == true);
        write_json(&suite.join("report.json"),&json!({"kind":"fox-native-host-process-crash-integration","valid":valid,"cases":outcomes,
            "compiledHostSha256":hash(include_bytes!("mod.rs")),"compiledFixtureSha256":hash(include_bytes!("native_crash_integration_tests.rs")),
            "limits":["process termination, not machine power loss or fsync proof","isolated real Host plus scripted Pi provider","no injected or rewritten Host receipts"]}));
        assert!(valid,"U06 crash evidence incomplete; inspect {}",suite.display());
    }
}
