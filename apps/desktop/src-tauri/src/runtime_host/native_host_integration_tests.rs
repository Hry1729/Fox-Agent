use super::*;
use fox_engine_protocol::{ExecutionAuthority,ExecutorRoute,ResourceExecutor,TimeBudgets};
use rusqlite::params;
use std::path::Path;
use std::time::{Duration,Instant};
use tauri::Manager;

const CHILD: &str = "FOX_TEST_NATIVE_HOST_INTEGRATION_CHILD";

fn wait_for(label: &str, mut complete: impl FnMut()->bool) {
    let deadline=Instant::now()+Duration::from_secs(30);
    while !complete() {
        assert!(Instant::now()<deadline,"timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_status(database: &Database, run: &str) -> String {
    database.with_connection(|connection|connection.query_row("SELECT status FROM runs WHERE id=?1",[run],|row|row.get(0))).unwrap()
}

fn native_run(database: &Database, conversation: &str, text: &str) -> StartRunResult {
    let started=database.create_run(conversation,text,None).unwrap();
    let (_root,mode)=database.conversation_project_access(conversation).unwrap().unwrap();
    assert_eq!(mode,"allow");
    let binding=database.freeze_default_run_control_for_engine(&started.run.id,"legacy",TimeBudgets::continuous(),"pi").unwrap();
    assert_eq!(binding.authority,ExecutionAuthority::Authoritative);
    assert_eq!(binding.executor_route,Some(ExecutorRoute::PiNative));
    assert_eq!(binding.read_only_executor,ResourceExecutor::Rust);
    started
}

fn pi_results(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap_or_default().lines().map(|line|serde_json::from_str::<Value>(line).unwrap())
        .filter_map(|entry|entry.get("message").cloned()).filter(|message|message["role"]=="toolResult").collect()
}

fn native_result(database: &Database, run: &str, call: &str) -> Value {
    let raw:String=database.with_connection(|connection|connection.query_row(
        "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",params![run,call],|row|row.get(0),
    )).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn start_native(host: &RuntimeHost, started: &StartRunResult, text: &str, root: &Path) {
    if let Err(error)=host.start_run(started,text,&[]) {
        let diagnostics=host.diagnostics();
        panic!("native Host start failed: {error}; evidence={}; state={}; last_error={:?}; stderr_tail={:?}",
            root.display(),diagnostics.state,diagnostics.last_error,diagnostics.stderr_tail);
    }
}

fn configure_model(database: &Database) {
    database.save_model_service(&crate::database::ModelServiceRecord {
        name:"isolated-native-faux".into(),base_url:"faux://fox-native-host-integration".into(),model_id:"fox-test".into(),api_type:"faux".into(),
        context_window:32768,max_output_tokens:4096,supports_image_input:false,enabled:true,credential_configured:false,
        connection_type:"local".into(),last_status:"unknown".into(),last_latency_ms:None,last_checked_at:None,created_at:1,updated_at:1,
    }).unwrap();
}

fn write_wrapper(root: &Path) -> PathBuf {
    let runtime=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../services/agent-runtime/src/pi-runtime.mjs");
    assert!(runtime.is_absolute() && runtime.is_file(),"real Pi runtime source is absent");
    let sidecar_binary=std::env::var_os("FOX_TEST_NATIVE_SIDECAR_BIN").map(|value|{
        let path=PathBuf::from(value);
        assert!(path.is_absolute() && path.is_file(),"FOX_TEST_NATIVE_SIDECAR_BIN must name an existing absolute executable file: {}",path.display());
        let resolved=path.canonicalize().expect("resolve FOX_TEST_NATIVE_SIDECAR_BIN");
        assert!(resolved.is_file(),"FOX_TEST_NATIVE_SIDECAR_BIN no longer names a file");
        println!("native Host integration sidecar executable: {}",resolved.display());
        resolved.to_str().expect("FOX_TEST_NATIVE_SIDECAR_BIN must be a Unicode path").to_owned()
    });
    let script=root.join("native-runtime-relay.mjs");
    let constants=format!("const runtime = {};\nconst root = {};\nconst sidecarBinary = {};\nconst raceVersion = {};\n",
        crate::canonical_json::to_string(&runtime.to_string_lossy()).unwrap(),
        crate::canonical_json::to_string(&root.to_string_lossy()).unwrap(),
        crate::canonical_json::to_string(&sidecar_binary).unwrap(),
        crate::canonical_json::to_string(&crate::tool_host::file_version(b"before-native-race")).unwrap());
    let relay=r#"
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { appendFileSync, existsSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
const nativePath=value=>value.startsWith('\\\\?\\UNC\\')?'\\\\'+value.slice(8):value.startsWith('\\\\?\\')?value.slice(4):value
const executable=sidecarBinary?nativePath(sidecarBinary):process.execPath
const args=sidecarBinary?[]:[nativePath(runtime)]
const cwd=dirname(sidecarBinary?executable:args[0])
writeFileSync(join(root,'sidecar-launch.json'),JSON.stringify({executable,args,cwd,compiledSidecar:sidecarBinary!==null}))
const child=spawn(executable,args,{cwd,windowsHide:true,shell:false,stdio:['pipe','pipe','pipe']})
const calls=new Map()
const executions=new Map()
const replayCalls=new Map()
const replays=[]
const held=[]
const log=(direction,frame)=>appendFileSync(join(root,'wire.jsonl'),JSON.stringify({direction,frame})+'\n')
const release=()=>{
  for(let index=held.length-1;index>=0;index--){
    if(existsSync(join(root,held[index].gate))){ child.stdin.write(JSON.stringify(held[index].frame)+'\n');held.splice(index,1) }
  }
  if(existsSync(join(root,'replay-race-failure'))){
    for(const replay of replays.splice(0)){
      replayCalls.set(replay.frame.id,replay.call)
      log('runtime-replay',replay.frame)
      process.stdout.write(JSON.stringify(replay.frame)+'\n')
    }
  }
}
const timer=setInterval(release,10)
createInterface({input:child.stdout}).on('line',line=>{
  const frame=JSON.parse(line)
  if(frame.kind==='request'&&frame.type==='tool_call.execute'){
    calls.set(frame.id,frame.payload.callId)
    executions.set(frame.id,frame)
    if(frame.payload.callId.startsWith('race-')||frame.payload.callId.startsWith('pure-read-'))writeFileSync(join(root,`execute-request-${frame.payload.callId}.json`),JSON.stringify(frame))
  }
  log('runtime',frame)
  process.stdout.write(line+'\n')
})
child.stderr.on('data',chunk=>{appendFileSync(join(root,'sidecar.stderr.log'),chunk);process.stderr.write(chunk)})
child.once('error',error=>{writeFileSync(join(root,'relay-error.log'),String(error.stack??error));process.stderr.write(String(error.stack??error)+'\n');process.exit(1)})
child.once('exit',(code,signal)=>{clearInterval(timer);process.exit(code??(signal?1:0))})
createInterface({input:process.stdin}).on('line',line=>{
  const frame=JSON.parse(line)
  if(frame.type==='prompt'){
    const text=frame.payload.text
    const service=frame.payload.modelService
    service.apiType='faux'
    service.fauxTokensPerSecond=10000
    if(text.includes('native-mixed'))service.fauxResponses=[{content:[
      {type:'toolCall',id:'mix-before',name:'read',arguments:{path:'note.txt'}},
      {type:'toolCall',id:'mix-write',name:'edit_file',arguments:{path:'note.txt',oldText:'before-native-write',newText:'after-native-write'}},
      {type:'toolCall',id:'mix-after',name:'read',arguments:{path:'note.txt'}}],stopReason:'toolUse'},'native mixed batch complete']
    else if(text==='native-pure-reads')service.fauxResponses=[{content:Array.from({length:6},(_,index)=>({type:'toolCall',id:`pure-read-${index}`,name:'read',arguments:{path:`pure-read-${index}.txt`,offset:0,limit:64}})),stopReason:'toolUse'},'native pure reads complete']
    else if(text==='native-file-race-a'||text==='native-file-race-b'){
      const suffix=text.endsWith('-a')?'a':'b'
      service.fauxResponses=[
        {content:[{type:'toolCall',id:`race-read-${suffix}`,name:'read',arguments:{path:'shared.txt'}}],stopReason:'toolUse'},
        {content:[{type:'toolCall',id:`race-write-${suffix}`,name:'edit_file',arguments:{path:'shared.txt',oldText:'before-native-race',newText:`winner-${suffix}`,expectedVersion:raceVersion}}],stopReason:'toolUse'},
        `native file race ${suffix} complete`]
    }
    else if(text.includes('cancel-target')){service.fauxTokensPerSecond=3;service.fauxResponses=['cancel target '+('waiting '.repeat(1000))]}
    else if(text.includes('survivor-next'))service.fauxResponses=[{content:[{type:'toolCall',id:'survivor-next-read',name:'read',arguments:{path:'note.txt'}}],stopReason:'toolUse'},'survivor next Run complete']
    else if(text.includes('survivor'))service.fauxResponses=[{content:[{type:'toolCall',id:'survivor-read',name:'read',arguments:{path:'note.txt'}}],stopReason:'toolUse'},'survivor complete']
    else throw new Error('unrecognized isolated integration prompt')
  }
  log('host',frame)
  const replayCall=replayCalls.get(frame.requestId)
  if(replayCall){
    writeFileSync(join(root,`replay-receipt-${replayCall}.json`),JSON.stringify(frame))
    replayCalls.delete(frame.requestId)
    return
  }
  const call=calls.get(frame.requestId)
  if(call&&(call.startsWith('mix-')||call==='survivor-read'||call.startsWith('race-')||call.startsWith('pure-read-'))){
    writeFileSync(join(root,`receipt-${call}.json`),JSON.stringify(frame))
    const gate=call.startsWith('pure-read-')?'release-pure-reads':call.startsWith('mix-')?'release-mixed':call==='survivor-read'?'release-survivor':call.startsWith('race-read-')?'release-race-reads':'release-race-writes'
    held.push({frame,gate})
    if(call.startsWith('race-write-')&&frame.payload?.isError===true){
      const original=executions.get(frame.requestId)
      if(!original)throw new Error('race failure has no original execution request')
      replays.push({call,frame:{...original,id:`${original.id}:native-race-replay`}})
    }
    release()
  }else child.stdin.write(JSON.stringify(frame)+'\n')
}).once('close',()=>child.stdin.end())
process.once('exit',()=>{try{child.kill()}catch{}})
"#;
    std::fs::write(&script,format!("{constants}{relay}")).unwrap();
    script
}

struct HostCleanup(RuntimeHost);
impl Drop for HostCleanup { fn drop(&mut self) { let _=self.0.shutdown(); } }

#[test]
fn native_host_real_sidecar_orders_commits_and_isolates_two_sessions() {
    if std::env::var_os(CHILD).is_none() {
        let mut child=Command::new(std::env::current_exe().unwrap())
            .args(["runtime_host::native_host_integration_tests::native_host_real_sidecar_orders_commits_and_isolates_two_sessions","--exact","--nocapture","--test-threads=1"])
            .env(CHILD,"1").env_remove("FOX_RUNTIME_EXECUTABLE").env_remove("FOX_RUNTIME_SCRIPT").env_remove("FOX_RUNTIME_MODE")
            .env("FOX_KERNEL_MODE","authoritative")
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline=Instant::now()+Duration::from_secs(150);
        while child.try_wait().unwrap().is_none() {
            if Instant::now()>=deadline { let _=child.kill();let _=child.wait();panic!("isolated native Host integration exceeded 150 seconds"); }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output=child.wait_with_output().unwrap();
        assert!(output.status.success(),"child stdout: {}\nchild stderr: {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        return;
    }
    let fixture_parent=std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root=fixture_parent.join(format!("fox-native-host-integration-{}",uuid::Uuid::new_v4()));
    println!("native Host integration fixture: {}",root.display());
    for directory in [&root,&root.join("sessions"),&root.join("attachments"),&root.join("skills"),&root.join("project-a"),&root.join("project-b")] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::env::set_var("FOX_RUNTIME_SCRIPT",write_wrapper(&root));
    std::fs::write(root.join("project-a/note.txt"),"before-native-write").unwrap();
    std::fs::write(root.join("project-b/note.txt"),"survivor-project").unwrap();
    let database=Database::open(root.join("facts.db")).unwrap();
    configure_model(&database);
    let a=database.create_conversation(database.default_agent_id(),Some("native A"),Some(root.join("project-a").to_str().unwrap()),Some("allow")).unwrap();
    let b=database.create_conversation(database.default_agent_id(),Some("native B"),Some(root.join("project-b").to_str().unwrap()),Some("allow")).unwrap();
    let mut context=tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create=false; }
    let app=tauri::Builder::default().any_thread().build(context).unwrap();
    let client=crate::yuxi::YuxiClient::new().unwrap();
    let host=RuntimeHost::new(app.handle().clone(),database.clone(),root.join("sessions"),root.join("attachments"),root.join("skills"),client.clone());
    let _cleanup=HostCleanup(host.clone());
    app.manage(crate::app_state::AppState::new(database.clone(),host.clone(),client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(),database.clone(),client),
        crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(&crate::local_knowledge_storage::resolve_startup_root(&root,None),None),root.clone(),root.join("skills")));

    let text="native-mixed: 把 note.txt 更新为 after-native-write，写前和写后读取文件";
    let mixed=native_run(&database,&a.id,text);
    start_native(&host,&mixed,text,&root);
    wait_for("all mixed Host responses held before delivery to Pi",||["mix-before","mix-write","mix-after"].iter().all(|call|root.join(format!("receipt-{call}.json")).is_file()));
    for call in ["mix-before","mix-write","mix-after"] {
        let frame:Value=serde_json::from_slice(&std::fs::read(root.join(format!("receipt-{call}.json"))).unwrap()).unwrap();
        let committed=native_result(&database,&mixed.run.id,call);
        assert_eq!(frame["payload"],committed,"Host must commit the exact response before transport delivery");
        assert_ne!(committed["isError"],true,"{call}: {committed}");
        assert!(database.get_runtime_tool_call(&mixed.run.id,call).unwrap().unwrap().result.is_some());
    }
    assert_eq!(native_result(&database,&mixed.run.id,"mix-before")["result"]["content"][0]["text"],"before-native-write");
    assert_eq!(native_result(&database,&mixed.run.id,"mix-after")["result"]["content"][0]["text"],"after-native-write");
    assert_eq!(std::fs::read_to_string(root.join("project-a/note.txt")).unwrap(),"after-native-write");
    let session_a=database.get_runtime_session(&a.id).unwrap().unwrap();
    assert!(session_a.is_native());
    assert!(pi_results(&session_a.native_path().unwrap()).is_empty(),"Pi has not received any held response");
    std::fs::write(root.join("release-mixed"),"release").unwrap();
    wait_for("mixed native Run completion",||run_status(&database,&mixed.run.id)=="completed");
    wait_for("mixed Run session ownership released",||host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&mixed.run.id).is_none());
    let results=pi_results(&session_a.native_path().unwrap());
    assert_eq!(results.iter().filter_map(|message|message["toolCallId"].as_str()).collect::<Vec<_>>(),vec!["mix-before","mix-write","mix-after"]);
    let retired_transport=host.state.lock().unwrap().worker.as_ref().unwrap().stdin.clone();
    host.stop_worker().unwrap();

    let cancel=native_run(&database,&a.id,"cancel-target");
    start_native(&host,&cancel,"cancel-target",&root);
    let survivor=native_run(&database,&b.id,"survivor");
    start_native(&host,&survivor,"survivor",&root);
    wait_for("second native session awaiting its own committed read response",||root.join("receipt-survivor-read.json").is_file());
    let pid={let state=host.state.lock().unwrap();let worker=state.worker.as_ref().unwrap();
        assert!(worker.sessions.by_run(&cancel.run.id).is_some());assert!(worker.sessions.by_run(&survivor.run.id).is_some());
        let id=worker.child.lock().unwrap().id();id};
    let session_b=database.get_runtime_session(&b.id).unwrap().unwrap();
    assert_ne!(session_a.runtime_session_id,session_b.runtime_session_id);
    assert_ne!(session_a.native_session_path,session_b.native_session_path);
    host.handle_worker_transport_crash(&retired_transport,"isolated stale transport failure".into());
    assert_eq!(host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id(),pid);
    assert_eq!(run_status(&database,&cancel.run.id),"running");
    assert_eq!(run_status(&database,&survivor.run.id),"running");
    let wrong_release=RuntimeEnvelope {protocol:fox_engine_protocol::PROTOCOL_NAME.into(),version:fox_engine_protocol::PROTOCOL_VERSION,
        kind:"event".into(),id:"test-wrong-generation-release".into(),r#type:"run_released".into(),request_id:None,
        conversation_id:Some(b.id.clone()),runtime_session_id:Some(session_b.runtime_session_id.clone()),run_id:Some(survivor.run.id.clone()),
        seq:None,timestamp:String::new(),payload:Some(json!({"contextGeneration":session_b.context_generation+1}))};
    assert_eq!(host.ensure_active_envelope_identity(&wrong_release).unwrap_err().0,"runtime.identity.generation_mismatch");
    assert!(host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&survivor.run.id).is_some());
    assert!(host.cancel_run(&cancel.run.id).unwrap());
    wait_for("cancel only the selected Run",||run_status(&database,&cancel.run.id)=="cancelled");
    wait_for("cancelled A Run session ownership released",||host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&cancel.run.id).is_none());
    assert_eq!(run_status(&database,&survivor.run.id),"running");
    assert_eq!(host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id(),pid);
    assert!(!root.join("release-survivor").exists());
    assert!(pi_results(&session_b.native_path().unwrap()).is_empty(),"B still waits for its held Host response");
    let survivor_receipt:Value=serde_json::from_slice(&std::fs::read(root.join("receipt-survivor-read.json")).unwrap()).unwrap();
    assert_eq!(survivor_receipt["payload"],native_result(&database,&survivor.run.id,"survivor-read"));
    let before_reset_a=database.get_runtime_session(&a.id).unwrap().unwrap();
    assert!(before_reset_a.is_native());
    let old_a_path=before_reset_a.native_path().unwrap();
    let old_a_bytes=std::fs::read(&old_a_path).expect("A has a persisted original native session");
    assert!(!old_a_bytes.is_empty());
    let reset_a=host.reset_conversation_context(&a.id).unwrap_or_else(|error|{
        let diagnostics=host.diagnostics();
        panic!("native Host reset failed: {error:?}; evidence={}; state={}; last_error={:?}; stderr_tail={:?}",
            root.display(),diagnostics.state,diagnostics.last_error,diagnostics.stderr_tail);
    });
    assert_ne!(reset_a.runtime_session_id,before_reset_a.runtime_session_id);
    assert_eq!(reset_a.context_generation,before_reset_a.context_generation+1);
    assert_ne!(reset_a.session_path,old_a_path);
    assert!(artifact_store::is_inside(&root.join("sessions"),&reset_a.session_path));
    let after_reset_a=database.get_runtime_session(&a.id).unwrap().unwrap();
    assert!(after_reset_a.is_native());
    assert_eq!(after_reset_a.runtime_session_id,reset_a.runtime_session_id);
    assert_eq!(after_reset_a.context_generation,reset_a.context_generation);
    assert_eq!(after_reset_a.native_path(),Some(reset_a.session_path.clone()));
    assert_eq!(host.live_runtime_session_id(&a.id).as_deref(),Some(reset_a.runtime_session_id.as_str()));
    assert_eq!(std::fs::read(&old_a_path).unwrap(),old_a_bytes,"reset preserves the original file byte for byte");
    let a_files=database.conversation_runtime_session_files(&a.id).unwrap();
    assert_eq!(a_files.len(),2,"A retains ownership of its original and reset sessions");
    let retired_a=a_files.iter().find(|file|!file.current&&file.context_generation==before_reset_a.context_generation).unwrap();
    assert_eq!(retired_a.runtime_session_id,before_reset_a.runtime_session_id);
    assert_eq!(retired_a.native_session_path,before_reset_a.native_session_path);
    let untouched_b=database.get_runtime_session(&b.id).unwrap().unwrap();
    assert_eq!(untouched_b.runtime_session_id,session_b.runtime_session_id);
    assert_eq!(untouched_b.context_generation,session_b.context_generation);
    assert_eq!(untouched_b.native_session_path,session_b.native_session_path);
    assert_eq!(run_status(&database,&survivor.run.id),"running");
    assert_eq!(host.live_runtime_session_id(&b.id).as_deref(),Some(session_b.runtime_session_id.as_str()));
    {
        let state=host.state.lock().unwrap();
        let worker=state.worker.as_ref().unwrap();
        assert_eq!(worker.child.lock().unwrap().id(),pid);
        let active_b=worker.sessions.by_run(&survivor.run.id).expect("reset A keeps B's original Run active");
        assert_eq!(active_b.conversation_id,b.id);
        assert_eq!(active_b.runtime_session_id,session_b.runtime_session_id);
        assert_eq!(active_b.generation,session_b.context_generation);
    }
    assert_eq!(native_result(&database,&survivor.run.id,"survivor-read"),survivor_receipt["payload"]);
    assert!(!root.join("release-survivor").exists());
    assert!(pi_results(&session_b.native_path().unwrap()).is_empty(),"reset A never delivers or replaces B's held response");
    std::fs::write(root.join("release-survivor"),"release").unwrap();
    wait_for("unaffected native session completion",||run_status(&database,&survivor.run.id)=="completed");
    wait_for("survivor Run session ownership released",||host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&survivor.run.id).is_none());
    assert_eq!(native_result(&database,&survivor.run.id,"survivor-read")["result"]["content"][0]["text"],"survivor-project");
    let next=native_run(&database,&b.id,"survivor-next");
    start_native(&host,&next,"survivor-next",&root);
    wait_for("next Run on the same native session",||run_status(&database,&next.run.id)=="completed");
    assert_eq!(native_result(&database,&next.run.id,"survivor-next-read")["result"]["content"][0]["text"],"survivor-project");
    let continued=database.get_runtime_session(&b.id).unwrap().unwrap();
    assert_eq!(continued.runtime_session_id,session_b.runtime_session_id);
    assert_eq!(continued.context_generation,session_b.context_generation);
    assert_eq!(host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id(),pid);
    wait_for("continued Run session ownership released",||host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&next.run.id).is_none());
    let rejected=native_run(&database,&a.id,"cancel-target for native rejection witness");
    start_native(&host,&rejected,"cancel-target for native rejection witness",&root);
    wait_for("native rejection target running",||run_status(&database,&rejected.run.id)=="running");
    {
        let state=host.state.lock().unwrap();
        let active_a=state.worker.as_ref().unwrap().sessions.by_run(&rejected.run.id).unwrap();
        assert_eq!(active_a.runtime_session_id,reset_a.runtime_session_id);
        assert_eq!(active_a.generation,reset_a.context_generation);
    }
    let resumed_a=database.get_runtime_session(&a.id).unwrap().unwrap();
    assert_eq!(resumed_a.runtime_session_id,reset_a.runtime_session_id);
    assert_eq!(resumed_a.context_generation,reset_a.context_generation);
    assert_eq!(resumed_a.native_path(),Some(reset_a.session_path.clone()));
    host.reject_runtime_run_event(&rejected.run.id,"runtime.test_native_rejection","isolated original protocol rejection");
    wait_for("native rejection settles and releases its original Run",||run_status(&database,&rejected.run.id)=="failed"
        && host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&rejected.run.id).is_none());
    let failure:(Option<String>,Option<String>)=database.with_connection(|connection|connection.query_row(
        "SELECT error_code,error_message FROM runs WHERE id=?1",[&rejected.run.id],|row|Ok((row.get(0)?,row.get(1)?)),
    )).unwrap();
    assert_eq!(failure.0.as_deref(),Some("runtime.test_native_rejection"));
    assert_eq!(failure.1.as_deref(),Some("isolated original protocol rejection"));
    assert_eq!(std::fs::read(&old_a_path).unwrap(),old_a_bytes,"the Run on A's new generation never rewrites its retired session");
    assert_eq!(run_status(&database,&next.run.id),"completed");
    assert_eq!(host.live_runtime_session_id(&b.id).as_deref(),Some(continued.runtime_session_id.as_str()));
    assert_eq!(host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id(),pid);
    host.shutdown().unwrap();
    println!("native Host integration evidence retained at {}",root.display());
}

#[test]
fn native_host_shared_file_conflict_preserves_the_failed_receipt() {
    const TEST: &str = "runtime_host::native_host_integration_tests::native_host_shared_file_conflict_preserves_the_failed_receipt";
    if std::env::var_os(CHILD).is_none() {
        let mut child=Command::new(std::env::current_exe().unwrap())
            .args([TEST,"--exact","--nocapture","--test-threads=1"])
            .env(CHILD,"1").env_remove("FOX_RUNTIME_EXECUTABLE").env_remove("FOX_RUNTIME_SCRIPT").env_remove("FOX_RUNTIME_MODE")
            .env("FOX_KERNEL_MODE","authoritative")
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline=Instant::now()+Duration::from_secs(150);
        while child.try_wait().unwrap().is_none() {
            if Instant::now()>=deadline {let _=child.kill();let _=child.wait();panic!("native shared-file integration exceeded 150 seconds");}
            std::thread::sleep(Duration::from_millis(20));
        }
        let output=child.wait_with_output().unwrap();
        assert!(output.status.success(),"child stdout: {}\nchild stderr: {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        return;
    }
    let fixture_parent=std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root=fixture_parent.join(format!("fox-native-file-race-{}",uuid::Uuid::new_v4()));
    println!("native shared-file Host fixture: {}",root.display());
    for name in ["sessions","attachments","skills","project"] {std::fs::create_dir_all(root.join(name)).unwrap();}
    std::env::set_var("FOX_RUNTIME_SCRIPT",write_wrapper(&root));
    let target=root.join("project/shared.txt");
    std::fs::write(&target,"before-native-race").unwrap();
    let database=Database::open(root.join("facts.db")).unwrap();
    configure_model(&database);
    let a=database.create_conversation(database.default_agent_id(),Some("shared file A"),root.join("project").to_str(),Some("allow")).unwrap();
    let b=database.create_conversation(database.default_agent_id(),Some("shared file B"),root.join("project").to_str(),Some("allow")).unwrap();
    let mut context=tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {window.create=false;}
    let app=tauri::Builder::default().any_thread().build(context).unwrap();
    let client=crate::yuxi::YuxiClient::new().unwrap();
    let host=RuntimeHost::new(app.handle().clone(),database.clone(),root.join("sessions"),root.join("attachments"),root.join("skills"),client.clone());
    let _cleanup=HostCleanup(host.clone());
    app.manage(crate::app_state::AppState::new(database.clone(),host.clone(),client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(),database.clone(),client),
        crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(&crate::local_knowledge_storage::resolve_startup_root(&root,None),None),root.clone(),root.join("skills")));
    let run_a=native_run(&database,&a.id,"native-file-race-a");
    let run_b=native_run(&database,&b.id,"native-file-race-b");
    start_native(&host,&run_a,"native-file-race-a",&root);
    start_native(&host,&run_b,"native-file-race-b",&root);
    wait_for("both sessions read the same file before either write",||["a","b"].iter().all(|suffix|root.join(format!("receipt-race-read-{suffix}.json")).is_file()));
    let read_json=|name:&str|serde_json::from_slice::<Value>(&std::fs::read(root.join(name)).unwrap()).unwrap();
    let session_a=database.get_runtime_session(&a.id).unwrap().unwrap();
    let session_b=database.get_runtime_session(&b.id).unwrap().unwrap();
    assert_ne!(session_a.runtime_session_id,session_b.runtime_session_id);
    assert_ne!(session_a.native_session_path,session_b.native_session_path);
    let pid=host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id();
    let version=crate::tool_host::file_version(b"before-native-race");
    for (suffix,started,conversation,session) in [("a",&run_a,&a,&session_a),("b",&run_b,&b,&session_b)] {
        let call=format!("race-read-{suffix}");
        let frame=read_json(&format!("receipt-{call}.json"));
        let committed=native_result(&database,&started.run.id,&call);
        assert_eq!(frame["payload"],committed);
        assert_eq!(committed["isError"],false,"{call}: {committed}");
        assert_eq!(committed["result"]["content"][0]["text"],"before-native-race");
        assert_eq!(committed["result"]["details"]["readVersion"],version);
        let request=read_json(&format!("execute-request-{call}.json"));
        assert_eq!(request["runId"],started.run.id);
        assert_eq!(request["conversationId"],conversation.id);
        assert_eq!(request["runtimeSessionId"],session.runtime_session_id);
        assert!(pi_results(&session.native_path().unwrap()).is_empty());
        assert_eq!(run_status(&database,&started.run.id),"running");
    }
    assert_eq!(database.run_control_binding(&run_a.run.id).unwrap().unwrap().permission.project_root,
        database.run_control_binding(&run_b.run.id).unwrap().unwrap().permission.project_root);
    assert_eq!(std::fs::read_to_string(&target).unwrap(),"before-native-race");
    assert!(!root.join("release-race-reads").exists());
    std::fs::write(root.join("release-race-reads"),"release both observed versions").unwrap();
    wait_for("both real Host writes settle before delivery to Pi",||["a","b"].iter().all(|suffix|root.join(format!("receipt-race-write-{suffix}.json")).is_file()));
    let outcomes=[("a",&run_a,&a,&session_a),("b",&run_b,&b,&session_b)].map(|(suffix,started,conversation,session)|{
        let call=format!("race-write-{suffix}");
        let receipt=read_json(&format!("receipt-{call}.json"));
        let committed=native_result(&database,&started.run.id,&call);
        assert_eq!(receipt["payload"],committed,"the real response must already be committed");
        let request=read_json(&format!("execute-request-{call}.json"));
        assert_eq!(request["runId"],started.run.id);
        assert_eq!(request["conversationId"],conversation.id);
        assert_eq!(request["runtimeSessionId"],session.runtime_session_id);
        assert_eq!(request["payload"]["input"]["expectedVersion"],version);
        assert_eq!(run_status(&database,&started.run.id),"running");
        let host_call=database.get_runtime_tool_call(&started.run.id,&call).unwrap().unwrap();
        assert_eq!(host_call.execution_location,"host");
        assert_eq!(host_call.status,if committed["isError"]==true {"failed"} else {"completed"});
        (suffix,started,call,committed)
    });
    let succeeded=outcomes.iter().filter(|(_,_,_,body)|body["isError"]==false).collect::<Vec<_>>();
    let failed=outcomes.iter().filter(|(_,_,_,body)|body["isError"]==true).collect::<Vec<_>>();
    assert_eq!(succeeded.len(),1,"exactly one original-version write may commit: {outcomes:?}");
    assert_eq!(failed.len(),1,"the other original-version write must report a conflict: {outcomes:?}");
    let (winner,_,_,_)=succeeded[0];
    let (_,loser,failed_call,failed_body)=failed[0];
    assert!(crate::canonical_json::to_string(failed_body).unwrap().contains("tool.file_conflict"),"{failed_body}");
    let winning_content=format!("winner-{winner}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(),winning_content);
    let original_modified=std::fs::metadata(&target).unwrap().modified().unwrap();
    let original_host=serde_json::to_value(database.get_runtime_tool_call(&loser.run.id,failed_call).unwrap().unwrap()).unwrap();
    let dispatch=fox_engine_protocol::encode_dispatch_id(&loser.run.id,failed_call).unwrap();
    let original_execution=serde_json::to_value(database.execution_receipt(&loser.run.id,&dispatch).unwrap()).unwrap();
    assert!(!root.join("release-race-writes").exists());
    std::fs::write(root.join("replay-race-failure"),"replay only the exact original failed call over the wire").unwrap();
    wait_for("failed call is replayed by the actual Host ingress",||root.join(format!("replay-receipt-{failed_call}.json")).is_file());
    let replay=read_json(&format!("replay-receipt-{failed_call}.json"));
    assert_eq!(&replay["payload"],failed_body,"replay returns the exact original committed failure");
    assert_eq!(&native_result(&database,&loser.run.id,failed_call),failed_body);
    assert_eq!(serde_json::to_value(database.get_runtime_tool_call(&loser.run.id,failed_call).unwrap().unwrap()).unwrap(),original_host);
    assert_eq!(serde_json::to_value(database.execution_receipt(&loser.run.id,&dispatch).unwrap()).unwrap(),original_execution);
    assert_eq!(std::fs::read_to_string(&target).unwrap(),winning_content);
    assert_eq!(std::fs::metadata(&target).unwrap().modified().unwrap(),original_modified);
    assert_eq!(host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id(),pid);
    std::fs::write(root.join("release-race-writes"),"deliver the two original committed outcomes").unwrap();
    wait_for("both native file-race Runs complete and release",||[&run_a.run.id,&run_b.run.id].iter().all(|run|
        run_status(&database,run)=="completed"&&host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(run).is_none()));
    for (suffix,started,conversation,session) in [("a",&run_a,&a,&session_a),("b",&run_b,&b,&session_b)] {
        let results=pi_results(&session.native_path().unwrap());
        let read_call=format!("race-read-{suffix}");
        let write_call=format!("race-write-{suffix}");
        assert_eq!(results.iter().filter_map(|message|message["toolCallId"].as_str()).collect::<Vec<_>>(),vec![read_call.as_str(),write_call.as_str()]);
        assert_eq!(results[1]["isError"],native_result(&database,&started.run.id,&write_call)["isError"]);
        let mapping=database.get_runtime_session(&conversation.id).unwrap().unwrap();
        assert_eq!(mapping.runtime_session_id,session.runtime_session_id);
        assert_eq!(mapping.context_generation,session.context_generation);
        assert_eq!(mapping.native_session_path,session.native_session_path);
        let other=if suffix=="a" {"b"} else {"a"};
        let foreign:usize=database.with_connection(|connection|connection.query_row(
            "SELECT COUNT(*) FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id IN (?2,?3)",
            params![started.run.id,format!("race-read-{other}"),format!("race-write-{other}")],|row|row.get(0))).unwrap();
        assert_eq!(foreign,0,"a native session cannot adopt the other Run's calls");
    }
    assert_eq!(std::fs::read_to_string(&target).unwrap(),winning_content);
    assert_eq!(host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id(),pid);
    host.shutdown().unwrap();
    println!("native shared-file Host evidence retained at {}",root.display());
}

#[test]
fn native_host_six_readers_overlap_real_file_loops_within_four_slots() {
    const TEST: &str = "runtime_host::native_host_integration_tests::native_host_six_readers_overlap_real_file_loops_within_four_slots";
    const FILE_BYTES: usize = 8 * 1024 * 1024;
    if std::env::var_os(CHILD).is_none() {
        let mut child=Command::new(std::env::current_exe().unwrap())
            .args([TEST,"--exact","--nocapture","--test-threads=1"])
            .env(CHILD,"1").env_remove("FOX_RUNTIME_EXECUTABLE").env_remove("FOX_RUNTIME_SCRIPT").env_remove("FOX_RUNTIME_MODE")
            .env("FOX_KERNEL_MODE","authoritative")
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline=Instant::now()+Duration::from_secs(150);
        while child.try_wait().unwrap().is_none() {
            if Instant::now()>=deadline {let _=child.kill();let _=child.wait();panic!("native pure-read integration exceeded 150 seconds");}
            std::thread::sleep(Duration::from_millis(20));
        }
        let output=child.wait_with_output().unwrap();
        assert!(output.status.success(),"child stdout: {}\nchild stderr: {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        return;
    }
    let fixture_parent=std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root=fixture_parent.join(format!("fox-native-pure-read-{}",uuid::Uuid::new_v4()));
    println!("native pure-read Host fixture: {}",root.display());
    for name in ["sessions","attachments","skills","project"] {std::fs::create_dir_all(root.join(name)).unwrap();}
    std::env::set_var("FOX_RUNTIME_SCRIPT",write_wrapper(&root));
    let targets=(0..6).map(|index|root.join(format!("project/pure-read-{index}.txt"))).collect::<Vec<_>>();
    let mut contents=vec![b'x';FILE_BYTES];
    let mut versions=Vec::new();
    let mut previews=Vec::new();
    for (index,path) in targets.iter().enumerate() {
        let prefix=format!("pure-read-{index}:\n");
        contents[..prefix.len()].copy_from_slice(prefix.as_bytes());
        std::fs::write(path,&contents).unwrap();
        versions.push(crate::tool_host::file_version(&contents));
        previews.push(String::from_utf8(contents[..64].to_vec()).unwrap());
    }
    let database=Database::open(root.join("facts.db")).unwrap();
    configure_model(&database);
    let conversation=database.create_conversation(database.default_agent_id(),Some("six actual Rust readers"),root.join("project").to_str(),Some("allow")).unwrap();
    let mut context=tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {window.create=false;}
    let app=tauri::Builder::default().any_thread().build(context).unwrap();
    let client=crate::yuxi::YuxiClient::new().unwrap();
    let host=RuntimeHost::new(app.handle().clone(),database.clone(),root.join("sessions"),root.join("attachments"),root.join("skills"),client.clone());
    let _cleanup=HostCleanup(host.clone());
    app.manage(crate::app_state::AppState::new(database.clone(),host.clone(),client.clone(),
        crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(),database.clone(),client),
        crate::model_service::ModelServiceClient::new().unwrap(),
        crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(&crate::local_knowledge_storage::resolve_startup_root(&root,None),None),root.clone(),root.join("skills")));
    let probe=crate::resource_gateway::test_probe::Probe::install(&targets,4,Duration::from_secs(5)).unwrap();
    let started=native_run(&database,&conversation.id,"native-pure-reads");
    start_native(&host,&started,"native-pure-reads",&root);
    wait_for("six actual reads committed before their responses reach Pi",||(0..6).all(|index|root.join(format!("receipt-pure-read-{index}.json")).is_file()));
    let snapshot=probe.snapshot().unwrap();
    let mut events=snapshot.intervals.iter().flat_map(|span|[(span.entered_ns,1_i32),(span.exited_ns,-1_i32)]).collect::<Vec<_>>();
    events.sort_unstable();
    let mut active=0_i32;
    let mut peak=0_i32;
    let mut minimum=0_i32;
    for (_,delta) in events {active+=delta;peak=peak.max(active);minimum=minimum.min(active);}
    let evidence=json!({
        "measurement":"Real Rust File::read loops; excludes the first-wave gate, claim, database, hashing and response delivery. This is not a physical-disk concurrency or performance measurement.",
        "fileBytes":FILE_BYTES,"peakReadLoops":peak,"minimumActive":minimum,"activeAfterAllIntervals":active,"snapshot":&snapshot,
    });
    let evidence_path=root.join("resource-read-intervals.json");
    std::fs::write(&evidence_path,crate::canonical_json::to_string(&evidence).unwrap()).unwrap();
    println!("native real-read intervals: {} peak={peak}",evidence_path.display());
    assert!(!snapshot.gate_failed,"first wave failed: {}",evidence_path.display());
    assert_eq!(snapshot.first_wave,4);
    assert_eq!(snapshot.opened_targets,6);
    assert_eq!(snapshot.intervals.len(),6,"each actual target must have one completed read interval");
    let released=snapshot.gate_released_ns.expect("four files must open before the timed read loops begin");
    let expected_targets=targets.iter().map(|path|path.canonicalize().unwrap()).collect::<std::collections::HashSet<_>>();
    let observed_targets=snapshot.intervals.iter().map(|span|span.target.clone()).collect::<std::collections::HashSet<_>>();
    assert_eq!(observed_targets,expected_targets);
    assert!(snapshot.intervals.iter().map(|span|&span.thread).collect::<std::collections::HashSet<_>>().len()>=2);
    for span in &snapshot.intervals {
        assert!(span.completed,"{span:?}");
        assert_eq!(span.bytes,FILE_BYTES as u64,"{span:?}");
        assert!(span.read_calls>=2,"a real nonempty read and EOF are required: {span:?}");
        assert!(span.opened_ns<=span.entered_ns&&released<=span.entered_ns,"the first-wave gate is outside the measured interval: {span:?}");
        assert!(span.exited_ns>span.entered_ns,"{span:?}");
    }
    assert_eq!(active,0);
    assert_eq!(minimum,0);
    assert!((2..=4).contains(&peak),"actual read-loop concurrency must be 2..=4: {}",evidence_path.display());
    let session=database.get_runtime_session(&conversation.id).unwrap().unwrap();
    let pid=host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id();
    for index in 0..6 {
        let call=format!("pure-read-{index}");
        let frame:Value=serde_json::from_slice(&std::fs::read(root.join(format!("receipt-{call}.json"))).unwrap()).unwrap();
        let committed=native_result(&database,&started.run.id,&call);
        assert_eq!(frame["payload"],committed);
        assert_eq!(committed["isError"],false,"{call}: {committed}");
        assert_eq!(committed["result"]["content"][0]["text"],previews[index]);
        assert_eq!(committed["result"]["details"]["readVersion"],versions[index]);
        let host_call=database.get_runtime_tool_call(&started.run.id,&call).unwrap().unwrap();
        assert_eq!(host_call.execution_location,"host");
        assert_eq!(host_call.status,"completed");
        assert!(host_call.result.is_some());
        let request:Value=serde_json::from_slice(&std::fs::read(root.join(format!("execute-request-{call}.json"))).unwrap()).unwrap();
        assert_eq!(request["runId"],started.run.id);
        assert_eq!(request["conversationId"],conversation.id);
        assert_eq!(request["runtimeSessionId"],session.runtime_session_id);
        assert_eq!(std::fs::metadata(&targets[index]).unwrap().len(),FILE_BYTES as u64);
    }
    let counts:(i64,i64)=database.with_connection(|connection|connection.query_row(
        "SELECT COUNT(*),COUNT(DISTINCT batch_id) FROM kernel_tool_calls WHERE run_id=?1",[&started.run.id],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
    assert_eq!(counts,(6,1));
    assert!(pi_results(&session.native_path().unwrap()).is_empty(),"the relay still holds every read result");
    std::fs::write(root.join("release-pure-reads"),"deliver all six committed read results").unwrap();
    wait_for("six-read native Run completes and releases",||run_status(&database,&started.run.id)=="completed"
        &&host.state.lock().unwrap().worker.as_ref().unwrap().sessions.by_run(&started.run.id).is_none());
    let results=pi_results(&session.native_path().unwrap());
    assert_eq!(results.iter().map(|message|message["toolCallId"].as_str().unwrap().to_owned()).collect::<Vec<_>>(),
        (0..6).map(|index|format!("pure-read-{index}")).collect::<Vec<_>>());
    assert!(results.iter().all(|message|message["isError"]==false));
    assert_eq!(host.state.lock().unwrap().worker.as_ref().unwrap().child.lock().unwrap().id(),pid);
    host.shutdown().unwrap();
    println!("native pure-read Host evidence retained at {}",root.display());
}
