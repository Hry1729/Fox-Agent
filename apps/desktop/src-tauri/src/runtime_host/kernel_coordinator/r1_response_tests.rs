//! R1 exercises the owning Host with a real isolated Node worker and local SSE.
//! These fixtures do not use an operator database, UI, or business model.
use super::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::time::{Duration,Instant};

struct AdvancingClock {base:i64,started:Instant}
impl Clock for AdvancingClock {
    fn now_monotonic_ms(&self)->i64 {self.base+self.started.elapsed().as_millis() as i64}
    fn now_wall_ms(&self)->i64 {self.now_monotonic_ms()}
}
fn clock()->AdvancingClock {AdvancingClock {base:crate::database::now_ms(),started:Instant::now()}}

struct Provider {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.store(true,Ordering::SeqCst);
        if let Some(thread)=self.thread.take() { let _=thread.join(); }
    }
}
fn provider(replies:Vec<(String,bool)>) -> Provider {
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();
    let address=listener.local_addr().unwrap(); listener.set_nonblocking(true).unwrap();
    let stop=Arc::new(AtomicBool::new(false)); let thread_stop=stop.clone();
    let requests=Arc::new(AtomicUsize::new(0)); let thread_requests=requests.clone();
    let thread=std::thread::spawn(move || {
        let mut completed_connections=Vec::new();
        let mut pending_connections:Vec<(std::net::TcpStream,Vec<u8>)>=Vec::new();
        while !thread_stop.load(Ordering::SeqCst) {
            // Undici can open an idle socket beside the real request. Do not
            // close it on a read timeout or block accept behind it.
            loop {
                match listener.accept() {
                    Ok((stream,_))=>{stream.set_nonblocking(true).unwrap();pending_connections.push((stream,Vec::new()));assert!(pending_connections.len()<=16);},
                    Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>break,
                    Err(error)=>panic!("{error}"),
                }
            }
            let mut retained=Vec::new();
            for (mut stream,mut bytes) in pending_connections.drain(..) {
                let mut closed=false;
                loop {
                    let mut buffer=[0u8;8192];
                    match stream.read(&mut buffer) {
                        Ok(0)=>{closed=true;break;},
                        Ok(n)=>{bytes.extend_from_slice(&buffer[..n]);assert!(bytes.len()<1_048_576);},
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>break,
                        Err(_)=>{closed=true;break;},
                    }
                }
                if closed {continue;}
                let complete=bytes.windows(4).position(|window|window==b"\r\n\r\n").and_then(|end| {
                    let headers=String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length=headers.lines().find_map(|line|line.strip_prefix("content-length:"))?.trim().parse::<usize>().ok()?;
                    (bytes.len()>=end+4+length).then_some((end,length))
                });
                let Some((end,length))=complete else {retained.push((stream,bytes));continue;};
                let _:Value=serde_json::from_slice(&bytes[end+4..end+4+length]).unwrap();
                let index=thread_requests.fetch_add(1,Ordering::SeqCst);
                let (body,interrupt)=replies.get(index).expect("unexpected provider replay");
                stream.set_nonblocking(false).unwrap();stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len()+if *interrupt {100}else{0},body).unwrap();stream.flush().unwrap();
                if !interrupt {completed_connections.push(stream);}
            }
            pending_connections=retained;
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    Provider {address,requests,stop,thread:Some(thread)}
}
fn chunk(delta:Value,finish:Option<&str>,usage:u64) -> String {
    let mut choice=json!({"index":0,"delta":delta});
    if let Some(finish)=finish {choice["finish_reason"]=json!(finish);}
    format!("data: {}\n\ndata: [DONE]\n\n",json!({"id":"r1-local","object":"chat.completion.chunk","created":1,
        "model":"r1-local","choices":[choice],"usage":{"prompt_tokens":3,"completion_tokens":usage,"total_tokens":3+usage}}))
}
fn tool(arguments:&str,name:&str) -> Value {
    json!({"role":"assistant","tool_calls":[{"index":0,"id":"r1-tool","type":"function",
        "function":{"name":name,"arguments":arguments}}]})
}
fn synthetic_worker(root:&std::path::Path,mode:&str) -> super::super::super::RuntimeCommand {
    let script=root.join("r1-isolated-worker.mjs");
    let body=r#"
import { createInterface } from 'node:readline'; import { writeFileSync } from 'node:fs';
const mode='__MODE__'; const send=(request,type,payload)=>process.stdout.write(JSON.stringify({
  protocol:request.protocol,version:request.version,kind:'response',type,requestId:request.id,
  runId:request.runId,conversationId:request.conversationId,runtimeSessionId:request.runtimeSessionId,payload})+'\n');
createInterface({input:process.stdin}).on('line',line=>{
 const request=JSON.parse(line);
 if(request.type==='kernel.initialize'){send(request,'kernel.ready',{singleUse:true,resourceExecution:false,automaticReplay:false,
  hostCompaction:true,roundLoop:request.payload.execution==='loop',adapterVersion:'__ADAPTER__'});return;}
 const frame=request.payload.initialModel??request.payload.batchResume;
 const turnId=frame.input?.turnId??frame.turnId,checkpointSeq=frame.checkpointSeq;
 if(mode==='cancel-late'){writeFileSync(new URL('./r1-dispatched',import.meta.url),'yes');setTimeout(()=>send(request,'request_failed',{
  responseDiagnostic:{schemaVersion:1,turnId,checkpointSeq,category:'length',stage:'tool_proposal',providerFinishReason:'length'}}),1000);return;}
 if(mode==='json'){process.stdout.write('private-r1-invalid-json\n');return;}
 if(mode==='oversized'){process.stdout.write('p'.repeat(1048600)+'\n');return;}
 if(mode==='exit'){process.exit(1);return;}
 if(mode==='old-failure'){send(request,'kernel.model_failure',{schemaVersion:1,runId:request.runId,turnId,checkpointSeq,
  category:'incomplete_response',httpStatus:null,retryAfterMs:null});return;}
 if(mode==='invalid-payload'){send(request,request.payload.initialModel?'kernel.model_response':'kernel.model_response',{response:{private:'private-r1-body'}});return;}
 if(mode==='malformed-failure'){send(request,'kernel.model_failure',{private:'private-r1-body',category:null});return;}
 send(request,mode==='unknown-protocol'?'private-r1-protocol':'request_failed',{message:'[kernel.permission_denied] private-r1-material',
  responseDiagnostic:{schemaVersion:1,turnId,checkpointSeq,source:'host_response_admission',category:'private-r1-category',
   stage:'private-r1-stage',providerFinishReason:'private-r1-reason',upstreamMessage:'private-r1-body',
   maxOutputTokens:9999999999999999,outputTokens:-1,usageRequestId:'private r1 credential'}});
});
"#;
    std::fs::write(&script,body.replace("__MODE__",mode).replace("__ADAPTER__",crate::kernel_model_config::KERNEL_MODEL_ADAPTER)).unwrap();
    super::super::super::RuntimeCommand {program:"node".into(),script:Some(script)}
}
fn configuration(address:SocketAddr,writable:bool) -> crate::kernel_model_config::KernelModelConfig {
    let mut config=worker_configuration();
    config.model_service=json!({"apiType":"openai-completions","modelId":"r1-local","maxOutputTokens":8192,
        "baseUrl":format!("http://{address}/v1")});
    if writable {config.proposal_tools.push(json!({"name":"write_file","description":"Host write proposal",
        "parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}));}
    config
}
fn scope(db:&Database,run:&str,config:&crate::kernel_model_config::KernelModelConfig) {
    let value=crate::database::KernelHostScope {schema_version:1,
        tool_names:config.proposal_tools.iter().map(|tool|tool["name"].as_str().unwrap().to_owned()).collect(),
        mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),
        knowledge_connection_hashes:Default::default(),office_tools:Default::default(),lifecycle_hooks:Vec::new()};
    db.freeze_kernel_host_scope(run,&value).unwrap();
}
fn facts(db:&Database,root:&std::path::Path,run:&str) -> (String,Vec<Value>,i64) {
    let connection=rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let state=db.kernel_build_full_snapshot(run).unwrap().state;
    let mut statement=connection.prepare("SELECT event_json FROM run_events WHERE run_id=?1 AND event_type='run.model_response_diagnostic' ORDER BY seq").unwrap();
    let diagnostics=statement.query_map([run],|row|row.get::<_,String>(0)).unwrap()
        .map(|row|serde_json::from_str(&row.unwrap()).unwrap()).collect();
    let writes=connection.query_row("SELECT COUNT(*) FROM managed_file_versions WHERE conversation_id=(SELECT conversation_id FROM runs WHERE id=?1)",[run],|row|row.get(0)).unwrap();
    (state,diagnostics,writes)
}
fn drive(db:&Database,root:&std::path::Path,run:&str,clock:&dyn Clock,cancellation:&CancellationRegistry,
    per_round:bool,execute:&(dyn Fn(&RunControlBinding,&kernel::OutboxEffect,&kernel::CancellationToken)->Result<(bool,Value),String>+Sync),
    runtime:&super::super::super::RuntimeCommand) {
    let ownership=super::super::super::kernel_host::acquire(root,run).unwrap();
    super::super::super::kernel_host::drive_with_actions_transport(&ownership,db,clock,cancellation,run,runtime,
        "r1-local-test-key",&Allow,execute,|_|Ok(()),|_|Ok(()),&|_|{},per_round).unwrap();
}

#[test]
fn r1_real_worker_host_persists_invalid_proposals_without_effects_or_replay() {
    for per_round in [false,true] {
        for (finish,arguments,name,category,code) in [
            (Some("length"),"{\"path\":\"private-r1-material\"","write_file","truncated_tool_proposal","kernel.model_response_incomplete"),
            (Some("tool_calls"),"{\"path\":\"private-r1-material\"","write_file","invalid_tool_arguments","kernel.model_response_invalid"),
            (None,"{\"path\":\"private-r1-material\"","write_file","truncated_tool_proposal","kernel.model_response_incomplete"),
            (Some("tool_calls"),"{}","private-r1-tool","protocol_invalid","kernel.model_response_invalid"),
        ] {
            let provider=provider(vec![(chunk(tool(arguments,name),finish,8192),false)]);
            let config=configuration(provider.address,true);
            let clock=clock();
            let (db,root,run)=fixture_with_budgets_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(2,2),true,TimeBudgets::default());
            scope(&db,&run,&config);
            let cancellation=CancellationRegistry::default();
            drive(&db,&root,&run,&clock,&cancellation,per_round,&|_,_,_|panic!("invalid proposal must not execute"),&real_worker_command());
            let (state,diagnostics,writes)=facts(&db,&root,&run);
            assert_eq!(state,"failed"); assert_eq!(writes,0); assert_eq!(diagnostics.len(),1);
            let diagnostic=&diagnostics[0];
            assert_eq!(diagnostic["worker"]["category"],category);
            assert_eq!(diagnostic["worker"]["providerFinishReason"],finish.unwrap_or("unknown"));
            assert_eq!(diagnostic["worker"]["maxOutputTokens"],8192);
            assert_eq!(diagnostic["worker"]["outputTokens"],8192);
            assert_eq!(diagnostic["resultAccepted"],false);
            assert_eq!(diagnostic["replayDecision"],"not_authorized_by_diagnostic");
            assert_eq!(diagnostic["attempts"],1);
            assert_eq!(provider.requests.load(Ordering::SeqCst),1);
            let connection=rusqlite::Connection::open(root.join("facts.db")).unwrap();
            let error:String=connection.query_row("SELECT error_code FROM runs WHERE id=?1",[&run],|row|row.get(0)).unwrap();
            assert_eq!(error,code);
            for table in ["kernel_tool_calls","kernel_approvals","kernel_execution_attempts","kernel_execution_credentials"] {
                let count:i64=connection.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE run_id=?1"),[&run],|row|row.get(0)).unwrap();
                assert_eq!(count,0,"{table}");
            }
            let recorded:String=connection.query_row("SELECT GROUP_CONCAT(event_json) FROM run_events WHERE run_id=?1 AND event_type='run.model_response_diagnostic'",[&run],|row|row.get(0)).unwrap();
            assert!(!recorded.contains("private-r1")); assert!(!recorded.contains("r1-local-test-key")); assert!(recorded.len()<4096);
            let outbox:(String,i64)=connection.query_row("SELECT status,attempts FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key='initial-model'",[&run],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
            assert_eq!(outbox,("leased".into(),1));
        }
    }
}

#[test]
fn r1_host_records_invalid_wire_old_worker_and_untrusted_errors_without_copying_text() {
    for per_round in [false,true] {
        for mode in ["json","oversized","unknown-protocol","malformed-failure","invalid-payload","worker-exception","old-failure","exit"] {
            let config=worker_configuration(); let clock=clock();
            let (db,root,run)=fixture_with_budgets_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,0),true,TimeBudgets::default());
            scope(&db,&run,&config);let cancellation=CancellationRegistry::default();
            drive(&db,&root,&run,&clock,&cancellation,per_round,&|_,_,_|panic!("invalid wire must not execute"),&synthetic_worker(&root,mode));
            let (state,diagnostics,writes)=facts(&db,&root,&run);
            assert_eq!(state,"failed","{mode}");assert_eq!(writes,0);assert_eq!(diagnostics.len(),1,"{mode} per_round={per_round}");
            let diagnostic=&diagnostics[0];
            assert_eq!(diagnostic["worker"]["providerFinishReason"],"unknown");
            assert_eq!(diagnostic["worker"]["source"],"worker_observation");
            assert_eq!(diagnostic["resultAccepted"],false);
            assert!(!diagnostic.to_string().contains("private-r1"));
            assert!(diagnostic.to_string().len()<4096);
        }
    }
}

#[test]
fn r1_diagnostic_sink_failure_preserves_the_existing_settled_retry_decision() {
    for per_round in [false,true] {
        let config=worker_configuration();let clock=clock();
        let (db,root,run)=fixture_with_budgets_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,1),true,TimeBudgets::default());
        scope(&db,&run,&config);let cancellation=CancellationRegistry::default();
        let connection=rusqlite::Connection::open(root.join("facts.db")).unwrap();
        connection.execute_batch("CREATE TRIGGER r1_reject_diagnostic BEFORE INSERT ON run_events WHEN NEW.event_type='run.model_response_diagnostic' BEGIN SELECT RAISE(ABORT,'synthetic diagnostic sink unavailable'); END;").unwrap();
        let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
        let runtime=synthetic_worker(&root,"old-failure");
        if per_round {coordinator.dispatch_initial_with_worker("r1-sink",&Allow,&runtime,"local-only").unwrap();}
        else {coordinator.dispatch_initial_live("r1-sink",&Allow,&runtime,"local-only",&|_,_,_|panic!("no tools"),&|_|Ok(()),&|_|Ok(())).unwrap();}
        assert_eq!(coordinator.snapshot().unwrap().state,"retry_scheduled");
        let category:String=connection.query_row("SELECT json_extract(payload_json,'$.failure.category') FROM kernel_events WHERE run_id=?1 AND event_type='engine.model_rejected'",[&run],|row|row.get(0)).unwrap();
        assert_eq!(category,"incomplete_response");
        assert_eq!(facts(&db,&root,&run).1.len(),0);
    }
}

#[test]
fn r1_cancelled_late_worker_does_not_persist_or_replay_a_diagnostic() {
    for per_round in [false,true] {
        let config=worker_configuration();let clock=clock();
        let (db,root,run)=fixture_with_budgets_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(2,2),true,TimeBudgets::default());
        scope(&db,&run,&config);let cancellation=CancellationRegistry::default();
        let runtime=synthetic_worker(&root,"cancel-late");
        std::thread::scope(|threads| {
            threads.spawn(|| {
                let deadline=std::time::Instant::now()+Duration::from_secs(5);
                while !root.join("r1-dispatched").exists() {assert!(std::time::Instant::now()<deadline);std::thread::sleep(Duration::from_millis(10));}
                db.queue_kernel_host_command(&run,None).unwrap();
                cancellation.request_run_cancel(&run);
            });
            drive(&db,&root,&run,&clock,&cancellation,per_round,&|_,_,_|panic!("cancelled proposal must not execute"),&runtime);
        });
        let (state,diagnostics,writes)=facts(&db,&root,&run);
        assert_eq!(state,"cancelled");assert!(diagnostics.is_empty());assert_eq!(writes,0);
        assert_eq!(db.kernel_rehydrate(&run).unwrap().unwrap().retry.turn_attempts,0);
    }
}

#[test]
fn r1_complete_stop_at_limit_and_complete_tools_keep_real_host_admission() {
    for per_round in [false,true] {
        for with_tool in [false,true] {
            let mut replies=Vec::new();
            if with_tool {replies.push((chunk(tool("{\"path\":\"proof.txt\"}","read"),Some("tool_calls"),10),false));}
            replies.push((chunk(json!({"role":"assistant","content":"Complete response at the token limit."}),Some("stop"),8192),false));
            let provider=provider(replies);
            let config=configuration(provider.address,false);
            let clock=clock();
            let (db,root,run)=fixture_with_budgets_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,0),true,TimeBudgets::default());
            scope(&db,&run,&config);
            let cancellation=CancellationRegistry::default(); let executions=AtomicUsize::new(0);
            drive(&db,&root,&run,&clock,&cancellation,per_round,&|_,_,_|{
                executions.fetch_add(1,Ordering::SeqCst);Ok((true,json!({"content":[{"type":"text","text":"Host-settled read"}]})))
            },&real_worker_command());
            let (state,diagnostics,writes)=facts(&db,&root,&run);
            let error:Option<String>=rusqlite::Connection::open(root.join("facts.db")).unwrap()
                .query_row("SELECT error_code FROM runs WHERE id=?1",[&run],|row|row.get(0)).unwrap();
            assert_eq!(state,"completed","per_round={per_round} with_tool={with_tool} error={error:?} diagnostic={diagnostics:?}"); assert!(diagnostics.is_empty()); assert_eq!(writes,0);
            assert_eq!(executions.load(Ordering::SeqCst),usize::from(with_tool));
            assert_eq!(provider.requests.load(Ordering::SeqCst),1+usize::from(with_tool));
        }
    }
}

#[test]
fn r1_length_and_interrupted_stream_retain_distinct_durable_evidence() {
    for per_round in [false,true] {
        for interrupt in [false,true] {
            let body=if interrupt {"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"private-r1-fragment\"}}]}\n\n".into()}
                else {chunk(json!({"role":"assistant","content":"private-r1-fragment"}),Some("length"),8192)};
            let provider=provider(vec![(body,interrupt)]);
            let config=configuration(provider.address,false); let clock=clock();
            let (db,root,run)=fixture_with_budgets_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,0),true,TimeBudgets::default());
            scope(&db,&run,&config); let cancellation=CancellationRegistry::default();
            drive(&db,&root,&run,&clock,&cancellation,per_round,&|_,_,_|panic!("partial text must not execute"),&real_worker_command());
            let (state,diagnostics,writes)=facts(&db,&root,&run);
            assert_eq!(state,"failed"); assert_eq!(writes,0); assert_eq!(diagnostics.len(),1);
            assert_eq!(diagnostics[0]["worker"]["providerFinishReason"],if interrupt {"unknown"} else {"length"});
            assert_eq!(diagnostics[0]["worker"]["category"],if interrupt {"stream_interrupted"} else {"length"});
            assert_eq!(diagnostics[0]["hostCategory"],"settled_model_failure");
            assert!(!diagnostics[0].to_string().contains("private-r1"));
            assert_eq!(provider.requests.load(Ordering::SeqCst),1);
        }
    }
}

#[test]
fn r1_failed_batch_binds_the_current_dispatch_without_replaying_settled_reads() {
    for per_round in [false,true] {
        let provider=provider(vec![
            (chunk(tool("{\"path\":\"proof.txt\"}","read"),Some("tool_calls"),8),false),
            (chunk(tool("{\"path\":\"private-r1-output\",\"content\":\"private-r1-body\"","write_file"),Some("tool_calls"),8192),false),
        ]);
        let config=configuration(provider.address,true);let clock=clock();
        let (db,root,run)=fixture_with_budgets_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(2,2),true,TimeBudgets::default());
        scope(&db,&run,&config);let cancellation=CancellationRegistry::default();let executions=AtomicUsize::new(0);
        drive(&db,&root,&run,&clock,&cancellation,per_round,&|_,effect,_|{
            let input:Value=serde_json::from_str(&effect.payload_json).unwrap();assert_eq!(input["tool"],"read");
            executions.fetch_add(1,Ordering::SeqCst);Ok((true,json!({"content":[{"type":"text","text":"settled input"}]})))
        },&real_worker_command());
        let (state,diagnostics,writes)=facts(&db,&root,&run);
        assert_eq!(state,"failed");assert_eq!(writes,0);assert_eq!(diagnostics.len(),1);
        assert_eq!(diagnostics[0]["effectType"],"deliver_tool_batch");
        assert!(diagnostics[0]["batchId"].as_str().is_some());
        assert_eq!(diagnostics[0]["worker"]["category"],"invalid_tool_arguments");
        assert_eq!(executions.load(Ordering::SeqCst),1);assert_eq!(provider.requests.load(Ordering::SeqCst),2);
        let connection=rusqlite::Connection::open(root.join("facts.db")).unwrap();
        let calls:i64=connection.query_row("SELECT COUNT(*) FROM kernel_tool_calls WHERE run_id=?1",[&run],|row|row.get(0)).unwrap();
        assert_eq!(calls,1);
        assert!(!root.join("private-r1-output").exists());
    }
}
