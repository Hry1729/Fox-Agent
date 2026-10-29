use super::{kernel_run_lock::KernelRunLock, RuntimeHost, SidecarExecutionWait};
use fox_engine_protocol::ExecutorRoute;

pub(super) struct NativeApprovalRecoveryScope {
    host: RuntimeHost,
    run: String,
    conversation: String,
    remove_session: bool,
    _run_lease: KernelRunLock,
    _session_lease: Option<KernelRunLock>,
}

impl NativeApprovalRecoveryScope {
    pub(super) fn host(&self) -> &RuntimeHost { &self.host }
}

impl Drop for NativeApprovalRecoveryScope {
    fn drop(&mut self) {
        if let Ok(mut state) = self.host.state.lock() {
            if let Some(worker) = state.worker.as_mut() {
                if worker.sessions.by_run(&self.run).is_some() {
                    worker.sessions.finish(&self.run);
                    if self.remove_session { let _ = worker.sessions.remove(&self.conversation); }
                }
            }
            state.cancellation.retire_run(&self.run);
            state.queue_changed.1.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{sidecar_sessions, PendingApproval, WorkerHandle};
    use crate::canonical_json::CanonicalJson;
    use crate::database::{ApprovalDecision, ChildRunBudget, CreateChildRunInput, CreateDigitalColleagueInput, Database};
    use fox_engine_protocol::{ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding, TimeBudgets};
    use rusqlite::params;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}, process::{Command, Stdio}, sync::{Arc, Mutex}, time::{Duration, Instant}};
    use tauri::Manager;
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;

    const CHILD: &str = "FOX_TEST_NATIVE_APPROVAL_SCOPE_CHILD";

    fn isolated(test: &str) -> bool {
        if std::env::var(CHILD).as_deref() == Ok(test) { return false; }
        let mut command=Command::new(std::env::current_exe().unwrap());
        command.args([test,"--exact","--nocapture","--test-threads=1"])
            .env(CHILD,test).env("FOX_KERNEL_MODE","authoritative").env("FOX_EXECUTOR_ROUTE","pi_native")
            .stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child=command.spawn().unwrap();
        let deadline=Instant::now()+Duration::from_secs(60);
        while child.try_wait().unwrap().is_none() {
            if Instant::now()>=deadline { let _=child.kill();let _=child.wait();panic!("native approval fixture exceeded 60 seconds: {test}"); }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output=child.wait_with_output().unwrap();
        assert!(output.status.success(),"child stdout: {}\nchild stderr: {}",
            String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        true
    }

    fn app() -> tauri::App {
        let mut context=tauri::generate_context!();
        for window in &mut context.config_mut().app.windows { window.create=false; }
        tauri::Builder::default().any_thread().build(context).unwrap()
    }

    struct Fixture { host: RuntimeHost, root: PathBuf }

    impl Fixture {
        fn new(app: &tauri::App, database: Database) -> Self {
            let parent=std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let root=parent.join(format!("fox-native-approval-scope-{}",uuid::Uuid::new_v4()));
            for name in ["sessions","attachments","skills","project"] { std::fs::create_dir_all(root.join(name)).unwrap(); }
            let host=RuntimeHost::new(app.handle().clone(),database,root.join("sessions"),root.join("attachments"),
                root.join("skills"),crate::yuxi::YuxiClient::new().unwrap());
            let mut command=Command::new("node");
            command.args(["-e","process.stdin.resume()"])
                .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            command.creation_flags(0x0800_0000);
            let mut child=command.spawn().expect("start the inert stdin-only worker");
            let stdin=Arc::new(Mutex::new(child.stdin.take().unwrap()));
            {
                let mut state=host.state.lock().unwrap();
                state.state="ready".into();
                let tools=["edit_file","task_repair_escalate_start"].map(|name| {
                    let (category,execution,approval)=fox_engine_protocol::canonical_runtime_tool_contract(name).unwrap();
                    json!({"name":name,"category":category,"execution":execution,"approval":approval})
                });
                state.capabilities=json!({"manifestVersion":fox_engine_protocol::CAPABILITY_MANIFEST_VERSION,
                    "streamingText":true,"cancellation":true,"reasoning":false,"sessionResume":true,"toolApproval":true,
                    "imageInput":false,"steering":false,"contextCompaction":true,"dynamicModelSwitch":false,"workLoop":true,"tools":tools});
                state.worker=Some(WorkerHandle { child:Arc::new(Mutex::new(child)),stdin,
                    pending:Arc::new(Mutex::new(HashMap::new())),sessions:sidecar_sessions::Sessions::default(),session_leases:HashMap::new() });
            }
            Self {host,root}
        }

        fn database(&self) -> &Database { &self.host.database }

        fn manage(&self, app: &tauri::App) {
            let database=self.database();
            let client=self.host.yuxi_client.clone();
            app.manage(crate::app_state::AppState::new(database.clone(),self.host.clone(),client.clone(),
                crate::yuxi::runtime::YuxiRuntimeHost::new(app.handle().clone(),database.clone(),client),
                crate::model_service::ModelServiceClient::new().unwrap(),
                crate::local_knowledge::LocalKnowledgeStore::open_for_startup_root(
                    &crate::local_knowledge_storage::resolve_startup_root(&self.root,None),None),
                self.root.clone(),self.root.join("skills")));
        }

        fn edit_input(&self, run: &str, call: &str) -> Value {
            let path=format!("{call}.txt");
            let before=format!("before-{call}");
            std::fs::write(self.root.join("project").join(&path),&before).unwrap();
            let binding=self.database().run_control_binding(run).unwrap().unwrap();
            let cancellation=crate::kernel::CancellationRegistry::default();
            cancellation.register_run(run).unwrap();
            let token=cancellation.tool_token(run,&format!("observe-{call}")).unwrap();
            super::super::managed_files::execute_observed_reader(self.database(),&binding,"read",&json!({"path":path}),
                &format!("observe-{call}"),&token,Duration::from_secs(5)).unwrap();
            json!({"path":path,"oldText":before,"newText":format!("after-{call}"),
                "expectedVersion":crate::tool_host::file_version(before.as_bytes())})
        }

        fn wait(&self, complete: impl Fn()->bool) {
            let deadline=Instant::now()+Duration::from_secs(15);
            while !complete() {
                assert!(Instant::now()<deadline,"native approval recovery did not finish; fixture: {}",self.root.display());
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let worker={let mut state=self.host.state.lock().unwrap();state.shutting_down=true;
                state.queue_changed.1.notify_all();state.worker.take()};
            if let Some(worker)=worker { let mut child=worker.child.lock().unwrap();let _=child.kill();let _=child.wait(); }
        }
    }

    fn make_native(database: &Database, run: &str, conversation: &str, root: Option<&Path>, profile: &str) -> String {
        database.freeze_run_execution_profile(run,profile).unwrap();
        let permission=FrozenPermission {mode:PermissionMode::Ask,project_root:root.map(|path|path.to_string_lossy().into_owned()),
            grants:Vec::new(),approval_epoch:None};
        database.freeze_run_control(&RunControlBinding {schema_version:fox_engine_protocol::CONTROL_SCHEMA_VERSION,
            run_id:run.into(),conversation_id:conversation.into(),engine_id:"pi".into(),execution_profile_id:profile.into(),
            authority:ExecutionAuthority::Authoritative,executor_route:Some(ExecutorRoute::PiNative),read_only_executor:ResourceExecutor::Rust,
            permission_snapshot_id:Database::run_control_permission_hash(&permission).unwrap(),permission,budgets:TimeBudgets::default()}).unwrap();
        let session=format!("approval-scope-{}",uuid::Uuid::new_v4());
        database.with_connection(|connection| {
            connection.execute("INSERT INTO runtime_sessions(id,conversation_id,runtime_type,runtime_session_id,status,created_at,updated_at,
                                format_version,context_generation,native_session_path) VALUES(?1,?2,'pi',?1,'ready',1,1,2,1,?3)",
                params![session,conversation,format!("{session}.jsonl")])?;
            connection.execute("UPDATE runs SET status='running',started_at=?2,runtime_session_id=?3 WHERE id=?1",
                params![run,crate::database::now_ms(),session])?;
            Ok(())
        }).unwrap();
        session
    }

    fn approval(database: &Database, run: &str, call: &str, tool: &str, input: &Value, category: &str, status: &str, claimed: bool) -> String {
        let body=input.canonical_string();
        let batch=format!("approval-batch-{call}");
        database.with_connection(|connection| {
            connection.execute("INSERT INTO kernel_tool_batches(batch_id,run_id,ordered_tool_call_ids_json,created_at) VALUES(?1,?2,?3,1)",
                params![batch,run,json!([call]).canonical_string()])?;
            connection.execute("INSERT INTO kernel_tool_calls(run_id,tool_call_id,batch_id,tool,source_order,canonical_input_json,state,
                                created_at,context_generation,input_digest,claimed_at) VALUES(?1,?2,?3,?4,0,?5,'running',1,1,?6,1)",
                params![run,call,batch,tool,body,format!("sha256:{:x}",Sha256::digest(body.as_bytes()))])?;
            Ok(())
        }).unwrap();
        let host=database.create_host_tool_call(run,call,tool,input,"pending",true).unwrap();
        let record=if category=="tool_execution" {
            let (root,_)=database.conversation_project_access(&host.conversation_id).unwrap().unwrap();
            let target=crate::tool_host::canonical_file_identity(Path::new(&root),input["path"].as_str().unwrap()).unwrap();
            let declared=input["expectedVersion"].as_str();
            let observed=database.host_observation_for_version(run,&target,declared).unwrap().unwrap();
            let baseline=crate::tool_host::write_baseline(Path::new(&target),declared,Some(&observed.version)).unwrap();
            let prepared=crate::tool_host::prepare_admitted_file_with_artifact_context(tool,input,&root,&baseline,
                database,Path::new(""),&host.conversation_id).unwrap();
            let hook=crate::lifecycle_hooks::before_tool(database,run,call,tool,input).unwrap();
            assert!(hook.blocked.is_none());
            let scope=if hook.requires_approval {None} else {super::super::host_permission_scope(tool,prepared.preview())};
            let mut request=serde_json::to_value(prepared.preview()).unwrap();
            super::super::attach_permission_scope(&mut request,scope.as_deref());
            database.create_approval(&host.id,&prepared.preview().summary,&request).unwrap()
        } else {
            database.create_approval(&host.id,"native scope fixture",&json!({"input":input})).unwrap()
        };
        if status!="pending" {
            database.resolve_approval(&record.id,if status=="approved" {ApprovalDecision::AllowOnce} else {ApprovalDecision::Deny}).unwrap();
        }
        database.with_connection(|connection| {
            if category=="tool_execution" {
                connection.execute("UPDATE approvals SET claimed_at=?2 WHERE id=?1",params![record.id,claimed.then_some(1_i64)])?;
            } else {
                connection.execute("UPDATE approvals SET category=?2,decision_json=?3,claimed_at=?4 WHERE id=?1",
                    params![record.id,category,(status!="pending").then(||json!({"approved":status=="approved","scope":"once","category":category}).canonical_string()),
                        claimed.then_some(1_i64)])?;
            }
            Ok(())
        }).unwrap();
        record.id
    }

    #[test]
    fn startup_recovers_only_decided_unclaimed_native_host_approvals() {
        if isolated("runtime_host::native_approval_scope::tests::startup_recovers_only_decided_unclaimed_native_host_approvals") { return; }
        let app=app();
        let fixture=Fixture::new(&app,Database::open(":memory:".into()).unwrap());
        println!("native startup approval fixture retained at {}",fixture.root.display());
        let database=fixture.database();
        fixture.manage(&app);
        let project=fixture.root.join("project");
        let conversation=database.create_conversation(database.default_agent_id(),Some("startup approvals"),project.to_str(),Some("ask")).unwrap().id;
        let run=database.create_run(&conversation,"settle original native approvals",None).unwrap().run.id;
        make_native(database,&run,&conversation,Some(&project),"durable_v2");
        let mut selected=Vec::new();
        let mut untouched=Vec::new();
        for category in ["tool_execution","task_repair_budget_override"] {
            let tool=if category=="tool_execution" {"edit_file"} else {"task_repair_escalate_start"};
            for status in ["approved","denied"] {
                let call=format!("{category}-{status}");
                // Invalid repair arguments exercise startup selection and the
                // original handler's refusal without starting another Run.
                let input=if category=="tool_execution" {fixture.edit_input(&run,&call)} else {json!({})};
                let id=approval(database,&run,&call,tool,&input,category,status,false);
                selected.push((call,id,category,status));
            }
            for (status,claimed) in [("pending",false),("approved",true)] {
                let call=format!("{category}-skip-{status}");
                let input=if category=="tool_execution" {fixture.edit_input(&run,&call)} else {json!({})};
                let id=approval(database,&run,&call,tool,&input,category,status,claimed);
                untouched.push((call,id,status,claimed));
            }
        }
        fixture.host.recover_native_approvals_detached().unwrap();
        fixture.wait(|| selected.iter().all(|(call,_,_,_)| database.with_connection(|connection|connection.query_row(
            "SELECT result_json IS NOT NULL FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",params![run,call],|row|row.get::<_,bool>(0))).unwrap())
            && fixture.host.state.lock().unwrap().worker.as_ref().unwrap().sessions.is_empty());
        for (call,id,category,status) in selected {
            let (body,claimed): (String,Option<i64>)=database.with_connection(|connection|connection.query_row(
                "SELECT k.result_json,a.claimed_at FROM kernel_tool_calls k JOIN tool_calls t ON t.run_id=k.run_id AND t.runtime_tool_call_id=k.tool_call_id
                 JOIN approvals a ON a.tool_call_id=t.id WHERE a.id=?1",[&id],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
            let result:Value=serde_json::from_str(&body).unwrap();
            if status=="approved" && category=="tool_execution" {
                assert_ne!(result["isError"],true,"{call}: {result}");
                assert!(claimed.is_some());
                let host=database.get_runtime_tool_call(&run,&call).unwrap().unwrap();
                assert_eq!(host.status,"completed");
                assert_eq!(host.execution_location,"host");
                let owner:Option<String>=database.with_connection(|connection|connection.query_row(
                    "SELECT claimed_by_run_id FROM approvals WHERE id=?1",[&id],|row|row.get(0))).unwrap();
                assert_eq!(owner.as_deref(),Some(run.as_str()));
                assert!(!database.claim_approved_tool_call(&run,&call).unwrap());
                let target=project.join(format!("{call}.txt"));
                let actual=std::fs::read_to_string(&target).unwrap();
                assert_eq!(actual,format!("after-{call}"),
                    "fixture={} target={} native={} host={host:?}",fixture.root.display(),target.display(),result);
            } else {
                assert_eq!(result["isError"],true,"{call}");
                assert!(claimed.is_none(),"{call} must not claim an execution");
                if status=="denied" {
                    assert!(result["error"].as_str().unwrap().contains("denied"));
                    assert_eq!(database.get_runtime_tool_call(&run,&call).unwrap().unwrap().status,"failed");
                    if category=="tool_execution" {
                        assert_eq!(std::fs::read_to_string(project.join(format!("{call}.txt"))).unwrap(),format!("before-{call}"));
                    }
                }
            }
        }
        for (call,id,status,claimed) in untouched {
            let row:(String,Option<i64>,Option<String>,String)=database.with_connection(|connection|connection.query_row(
                "SELECT a.status,a.claimed_at,k.result_json,t.status FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                 JOIN kernel_tool_calls k ON k.run_id=t.run_id AND k.tool_call_id=t.runtime_tool_call_id WHERE a.id=?1",
                [&id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))).unwrap();
            assert_eq!(row,(status.into(),claimed.then_some(1),None,"pending".into()),"{call}");
            if call.starts_with("tool_execution-") {
                assert_eq!(std::fs::read_to_string(project.join(format!("{call}.txt"))).unwrap(),format!("before-{call}"));
            }
        }
    }

    #[test]
    fn approval_recovery_preserves_failed_file_execution_and_never_replays_it() {
        if isolated("runtime_host::native_approval_scope::tests::approval_recovery_preserves_failed_file_execution_and_never_replays_it") { return; }
        let app=app();
        let fixture=Fixture::new(&app,Database::open(":memory:".into()).unwrap());
        println!("native failed approval fixture retained at {}",fixture.root.display());
        fixture.manage(&app);
        let database=fixture.database();
        let project=fixture.root.join("project");
        let conversation=database.create_conversation(database.default_agent_id(),Some("failed approval recovery"),project.to_str(),Some("ask")).unwrap().id;
        let run=database.create_run(&conversation,"perform only the original approved edit",None).unwrap().run.id;
        make_native(database,&run,&conversation,Some(&project),"durable_v2");
        let call="approved-edit-with-unavailable-backup";
        let input=fixture.edit_input(&run,call);
        let approval_id=approval(database,&run,call,"edit_file",&input,"tool_execution","approved",false);
        let backup_root=fixture.host.managed_files_dir().to_path_buf();
        std::fs::write(&backup_root,b"a file cannot serve as the managed backup directory").unwrap();
        let stdin=fixture.host.state.lock().unwrap().worker.as_ref().unwrap().stdin.clone();
        assert!(super::super::native_tool_batch::recover_native_approval(&fixture.host,&stdin,&approval_id).unwrap());
        let body:String=database.with_connection(|connection|connection.query_row(
            "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",params![run,call],|row|row.get(0))).unwrap();
        let response:Value=serde_json::from_str(&body).unwrap();
        let host=database.get_runtime_tool_call(&run,call).unwrap().unwrap();
        let target=project.join(format!("{call}.txt"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(),format!("before-{call}"));
        assert_eq!(response["isError"],true,"{response}");
        assert_eq!(response["result"]["isError"],true,"{response}");
        assert!(response["result"]["details"]["underlyingMessage"].as_str().unwrap()
            .contains("cannot create managed-file backup directory"),"{response}");
        assert_eq!(host.status,"failed");
        assert!(host.error_message.as_deref().is_some_and(|message|!message.is_empty()));
        assert_eq!(host.result.as_ref(),Some(&response["result"]));
        let claimed:Option<String>=database.with_connection(|connection|connection.query_row(
            "SELECT claimed_by_run_id FROM approvals WHERE id=?1",[&approval_id],|row|row.get(0))).unwrap();
        assert_eq!(claimed.as_deref(),Some(run.as_str()));
        let dispatch=fox_engine_protocol::encode_dispatch_id(&run,call).unwrap();
        let receipt=database.execution_receipt(&run,&dispatch).unwrap().unwrap();
        assert_eq!(receipt.execution_started,fox_engine_protocol::TriState::False);
        assert_eq!(receipt.external_effect,fox_engine_protocol::SideEffectState::None);
        assert_eq!(receipt.stage,fox_engine_protocol::ExecutionStage::FilePrepared);
        assert!(receipt.reason_code.is_some());
        let receipt_json=serde_json::to_value(&receipt).unwrap();
        assert_eq!(response["result"]["details"]["executionReceipt"],receipt_json);
        let before=crate::canonical_json::to_string(&host).unwrap();

        // Make a replay capable of changing the target, then prove both the
        // direct recovery and the startup selector retain the original failure.
        std::fs::remove_file(&backup_root).unwrap();
        assert!(super::super::native_tool_batch::recover_native_approval(&fixture.host,&stdin,&approval_id).is_err());
        fixture.host.recover_native_approvals_detached().unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(),format!("before-{call}"));
        assert!(!backup_root.exists());
        assert_eq!(crate::canonical_json::to_string(&database.get_runtime_tool_call(&run,call).unwrap().unwrap()).unwrap(),before);
        let retained:String=database.with_connection(|connection|connection.query_row(
            "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id=?2",params![run,call],|row|row.get(0))).unwrap();
        assert_eq!(retained,body);
        assert_eq!(serde_json::to_value(database.execution_receipt(&run,&dispatch).unwrap().unwrap()).unwrap(),receipt_json);
    }

    fn scoped_agents(database: &Database) {
        database.with_connection(|connection| {
            connection.execute("UPDATE agents SET is_builtin=0,package_manifest_json=?2 WHERE id=?1",
                params![database.default_agent_id(),json!({"allowedTools":["read","ls","grep","call_mcp_tool"],
                    "mcpServers":["mcp-shared","mcp-assistant"]}).canonical_string()])?;
            connection.execute("UPDATE agents SET is_builtin=0,package_manifest_json=?1 WHERE id='fox-debugger'",
                params![json!({"allowedTools":["read","grep","call_mcp_tool"],"mcpServers":["mcp-shared","mcp-expert"]}).canonical_string()])?;
            Ok(())
        }).unwrap();
    }

    fn set(values: &[&str]) -> Option<HashSet<String>> { Some(values.iter().map(|value|(*value).into()).collect()) }

    #[test]
    fn approval_recovery_rebuilds_child_and_digital_tool_and_mcp_scopes() {
        if isolated("runtime_host::native_approval_scope::tests::approval_recovery_rebuilds_child_and_digital_tool_and_mcp_scopes") { return; }
        let app=app();
        let fixture=Fixture::new(&app,Database::open(":memory:".into()).unwrap());
        let database=fixture.database();
        scoped_agents(database);
        let parent=database.create_conversation(database.default_agent_id(),Some("scoped parent"),None,Some("ask")).unwrap().id;
        database.bind_conversation_expert(&parent,"fox-debugger","native_scope_fixture").unwrap();
        let parent_run=database.create_run(&parent,"delegate a bounded read",None).unwrap().run.id;
        let allowed=vec!["read".into(),"ls".into(),"write_file".into(),"call_mcp_tool".into()];
        let budget=ChildRunBudget {max_duration_ms:60_000,max_total_tokens:4_096,max_output_tokens:1_024,max_tool_calls:8};
        let (started,_,_)=database.create_child_run(CreateChildRunInput {parent_run_id:&parent_run,tool_call_id:"delegate",
            worker_agent_id:database.default_agent_id(),objective:"read with the parent's limits",context:"scope fixture",budget:&budget,
            team_run_id:None,team_member_id:None,allowed_tools:Some(&allowed)}).unwrap();
        make_native(database,&started.run.id,&started.run.conversation_id,None,"legacy");
        {
            let scope=fixture.host.prepare_native_approval_recovery(&started.run.id).unwrap();
            assert_eq!(scope.host().tool_allowlist_override,set(&["read","call_mcp_tool"]));
            assert_eq!(scope.host().mcp_server_scope_override,set(&["mcp-shared"]));
            assert!(scope.host().ensure_delegated_tool_allowed("read").is_ok());
            for denied in ["ls","grep","write_file"] { assert!(scope.host().ensure_delegated_tool_allowed(denied).is_err(),"{denied}"); }
            assert!(fixture.host.tool_allowlist_override.is_none());
            assert!(fixture.host.mcp_server_scope_override.is_none());
        }
        assert!(fixture.host.state.lock().unwrap().worker.as_ref().unwrap().sessions.is_empty());
        assert!(KernelRunLock::acquire(&fixture.host.sessions_dir,&started.run.id).is_ok());

        let colleague=database.create_digital_colleague(&CreateDigitalColleagueInput {name:"Scoped recovery".into(),expert_id:"fox-debugger".into(),
            objective:"read within the bound MCP scope".into(),project_id:None,project_root:None,knowledge_references:Vec::new(),
            max_runs_per_day:2,max_tokens_per_day:10_000,max_duration_ms:60_000,max_output_tokens:1_024,max_tool_calls:8}).unwrap();
        database.update_conversation_permission_mode(&colleague.conversation_id,"ask").unwrap();
        let accepted=database.accept_digital_colleague_trigger(&colleague.id,"manual",None,"native-scope-test",&json!({}),None,"test").unwrap();
        let started=accepted.prepared.expect("digital colleague trigger is admitted").started;
        make_native(database,&started.run.id,&started.run.conversation_id,None,"legacy");
        let scope=fixture.host.prepare_native_approval_recovery(&started.run.id).unwrap();
        assert_eq!(scope.host().tool_allowlist_override,set(&["read","grep","call_mcp_tool"]));
        assert_eq!(scope.host().mcp_server_scope_override,set(&["mcp-shared"]));
        assert!(scope.host().ensure_delegated_tool_allowed("grep").is_ok());
        assert!(scope.host().ensure_delegated_tool_allowed("ls").is_err());
        assert!(scope.host().ensure_delegated_tool_allowed("write_file").is_err());
    }

    #[test]
    fn startup_retires_native_kernel_commands_but_keeps_the_approval_pending() {
        if isolated("runtime_host::native_approval_scope::tests::startup_retires_native_kernel_commands_but_keeps_the_approval_pending") { return; }
        let app=app();
        let fixture=Fixture::new(&app,Database::open(":memory:".into()).unwrap());
        fixture.manage(&app);
        let database=fixture.database();
        let conversation=database.create_conversation(database.default_agent_id(),Some("retire stale commands"),None,Some("ask")).unwrap().id;
        let run=database.create_run(&conversation,"retire the old Kernel command",None).unwrap().run.id;
        make_native(database,&run,&conversation,None,"legacy");
        let approval_id=approval(database,&run,"native-retire","task_repair_escalate_start",&json!({}),"task_repair_budget_override","pending",false);
        let runtime_call:String=database.with_connection(|connection|connection.query_row(
            "SELECT t.runtime_tool_call_id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id WHERE a.id=?1",
            [&approval_id],|row|row.get(0))).unwrap();
        database.with_connection(|connection|connection.execute(
            "INSERT INTO kernel_host_commands(run_id,command_key,kind,tool_call_id,decision,status,created_at,policy_version)
             VALUES(?1,'stale-native-approval','approval',?2,'denied','pending',1,NULL)",
            params![&run,&runtime_call])).unwrap();
        assert_eq!(fixture.host.retire_native_approval_commands().unwrap(),1);
        let (command_status,reason,approval_status):(String,Option<String>,String)=database.with_connection(|connection|connection.query_row(
            "SELECT c.status,c.resolution_reason,a.status FROM kernel_host_commands c
             JOIN approvals a ON a.tool_call_id=(SELECT id FROM tool_calls WHERE run_id=c.run_id AND runtime_tool_call_id=c.tool_call_id)
             WHERE c.run_id=?1",
            [&run],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
        assert_eq!(command_status,"superseded");
        assert!(reason.unwrap().contains("pi_native"));
        assert_eq!(approval_status,"pending","the decision still belongs to the user");
        assert_eq!(fixture.host.retire_native_approval_commands().unwrap(),0,"idempotent");
        assert!(fixture.host.resolve_approval(&approval_id,ApprovalDecision::Deny).unwrap());
        let denied:String=database.with_connection(|connection|connection.query_row(
            "SELECT status FROM approvals WHERE id=?1",[&approval_id],|row|row.get(0))).unwrap();
        assert_eq!(denied,"denied");
    }

    #[test]
    fn host_resolve_native_approval_with_waiter_never_enters_the_kernel_queue() {
        if isolated("runtime_host::native_approval_scope::tests::host_resolve_native_approval_with_waiter_never_enters_the_kernel_queue") { return; }
        let app=app();
        let fixture=Fixture::new(&app,Database::open(":memory:".into()).unwrap());
        fixture.manage(&app);
        let database=fixture.database();
        let conversation=database.create_conversation(database.default_agent_id(),Some("native approval route"),None,Some("ask")).unwrap().id;
        let run=database.create_run(&conversation,"resolve the native approval through the Host",None).unwrap().run.id;
        make_native(database,&run,&conversation,None,"legacy");
        let approval_id=approval(database,&run,"native-waiter","task_repair_escalate_start",&json!({}),"task_repair_budget_override","pending",false);
        let (sender,receiver)=std::sync::mpsc::channel();
        fixture.host.state.lock().unwrap().pending_approvals.lock().unwrap().insert(
            approval_id.clone(),PendingApproval{run_id:run.clone(),sender});
        assert!(fixture.host.resolve_approval(&approval_id,ApprovalDecision::Deny).unwrap());
        assert_eq!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(),false);
        let (status,decision):(String,Option<String>)=database.with_connection(|connection|connection.query_row(
            "SELECT status,decision_json FROM approvals WHERE id=?1",[&approval_id],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
        assert_eq!(status,"denied");
        assert!(decision.unwrap().contains("\"approved\":false"));
        let queued:i64=database.with_connection(|connection|connection.query_row(
            "SELECT COUNT(*) FROM kernel_host_commands",[],|row|row.get(0))).unwrap();
        assert_eq!(queued,0,"a native approval must not enter the Kernel command queue");
        assert!(database.kernel_host_approval_target(&approval_id).unwrap().is_none());
    }

    #[test]
    fn host_resolve_native_approval_without_waiter_recovers_once() {
        if isolated("runtime_host::native_approval_scope::tests::host_resolve_native_approval_without_waiter_recovers_once") { return; }
        let app=app();
        let fixture=Fixture::new(&app,Database::open(":memory:".into()).unwrap());
        fixture.manage(&app);
        let database=fixture.database();
        let conversation=database.create_conversation(database.default_agent_id(),Some("native approval recovery"),None,Some("ask")).unwrap().id;
        let run=database.create_run(&conversation,"recover the native approval exactly once",None).unwrap().run.id;
        make_native(database,&run,&conversation,None,"legacy");
        let approval_id=approval(database,&run,"native-no-waiter","task_repair_escalate_start",&json!({}),"task_repair_budget_override","pending",false);
        assert!(fixture.host.resolve_approval(&approval_id,ApprovalDecision::Deny).unwrap());
        let status:String=database.with_connection(|connection|connection.query_row(
            "SELECT status FROM approvals WHERE id=?1",[&approval_id],|row|row.get(0))).unwrap();
        assert_eq!(status,"denied");
        assert!(database.kernel_host_approval_target(&approval_id).unwrap().is_none());
        let command_count:i64=database.with_connection(|connection|connection.query_row(
            "SELECT COUNT(*) FROM kernel_host_commands",[],|row|row.get(0))).unwrap();
        assert_eq!(command_count,0);
    }

    #[test]
    fn approval_recovery_refuses_terminal_runs_and_retired_native_sessions() {
        if isolated("runtime_host::native_approval_scope::tests::approval_recovery_refuses_terminal_runs_and_retired_native_sessions") { return; }
        let app=app();
        let fixture=Fixture::new(&app,Database::open(":memory:".into()).unwrap());
        let database=fixture.database();
        let conversation=database.create_conversation(database.default_agent_id(),Some("recovery identity"),None,Some("ask")).unwrap().id;
        let run=database.create_run(&conversation,"recover only the original live generation",None).unwrap().run.id;
        make_native(database,&run,&conversation,None,"legacy");
        for status in ["completed","failed","cancelled","interrupted"] {
            database.with_connection(|connection|connection.execute("UPDATE runs SET status=?2 WHERE id=?1",params![run,status])).unwrap();
            let error=fixture.host.prepare_native_approval_recovery(&run).err().expect("terminal Run must be refused");
            assert!(error.contains("terminal Run"),"{status}: {error}");
        }
        database.with_connection(|connection| {
            connection.execute("UPDATE runs SET status='running' WHERE id=?1",[&run])?;
            connection.execute("UPDATE runtime_sessions SET runtime_session_id=runtime_session_id || '-replacement',context_generation=2 WHERE conversation_id=?1",[&conversation])?;
            Ok(())
        }).unwrap();
        let error=fixture.host.prepare_native_approval_recovery(&run).err().expect("retired native identity must be refused");
        assert!(error.contains("retired session"),"{error}");
        let state=fixture.host.state.lock().unwrap();
        assert!(state.worker.as_ref().unwrap().sessions.is_empty());
        assert!(state.cancellation.run_token(&run).is_err());
    }
}

impl RuntimeHost {
    pub(super) fn prepare_native_approval_recovery(&self, run: &str) -> Result<NativeApprovalRecoveryScope, String> {
        let binding = self.database.run_control_binding(run)?.ok_or("native recovery has no frozen Run")?;
        if binding.executor_route != Some(ExecutorRoute::PiNative) { return Err("approval belongs to another executor".into()); }
        let mapping = self.database.get_runtime_session(&binding.conversation_id)?.ok_or("native recovery session is missing")?;
        let (status, recorded): (String, Option<String>) = self.database.with_connection(|connection| connection.query_row(
            "SELECT status,runtime_session_id FROM runs WHERE id=?1", [run], |row| Ok((row.get(0)?,row.get(1)?)),
        ))?;
        if status != "running" || !mapping.is_native() || mapping.context_generation < 1
            || recorded.as_deref() != Some(mapping.runtime_session_id.as_str()) {
            return Err("native recovery cannot adopt a terminal Run or a retired session".into());
        }
        let mut scoped_host = self.clone();
        if let Some(child) = self.database.child_run(run)? {
            let parent_conversation = self.database.authoritative_run_conversation_id(&child.parent_run_id).map_err(|error| error.to_string())?;
            let (tools, mcp) = super::effective_conversation_delegation_scope(&self.database, &parent_conversation)?;
            scoped_host.tool_allowlist_override = super::intersect_optional_scopes(tools,
                child.allowed_tools.as_ref().map(|tools| tools.iter().cloned().collect()));
            scoped_host.mcp_server_scope_override = mcp;
        } else if self.database.is_digital_colleague_run(run)? {
            let (tools, mcp) = super::effective_conversation_delegation_scope(&self.database, &binding.conversation_id)?;
            scoped_host.tool_allowlist_override = tools;
            scoped_host.mcp_server_scope_override = mcp;
        }
        let run_lease = KernelRunLock::acquire(&self.sessions_dir, run)?;
        let (remove_session, session_lease) = {
            let mut state = self.state.lock().map_err(|_| "runtime state lock is poisoned")?;
            if state.shutting_down || state.native_run_leases.contains_key(run)
                || state.pending_approvals.lock().map_err(|_| "approval map is poisoned")?.values().any(|pending| pending.run_id == run) {
                return Err("native recovery cannot replace a live approval owner".into());
            }
            let worker = state.worker.as_mut().ok_or("native recovery worker is unavailable")?;
            let remove_session = worker.sessions.by_conversation(&binding.conversation_id).is_none();
            let session_lease = if worker.session_leases.contains_key(&binding.conversation_id) { None } else {
                Some(KernelRunLock::acquire(&self.sessions_dir, &format!("native-session:{}", binding.conversation_id))?)
            };
            worker.sessions.reserve_waiting(&binding.conversation_id, &mapping.runtime_session_id, mapping.context_generation, run)?;
            if let Err(error) = state.cancellation.register_run(run) {
                let worker = state.worker.as_mut().unwrap();
                worker.sessions.finish(run);
                if remove_session { let _ = worker.sessions.remove(&binding.conversation_id); }
                return Err(error);
            }
            (remove_session, session_lease)
        };
        let scope = NativeApprovalRecoveryScope { host:scoped_host,run:run.to_owned(),conversation:binding.conversation_id,
            remove_session,_run_lease:run_lease,_session_lease:session_lease };
        SidecarExecutionWait { state:self.state.clone(),run_id:run.to_owned() }.resume()?;
        Ok(scope)
    }

    pub(crate) fn retire_native_approval_commands(&self) -> Result<usize, String> {
        self.database.supersede_pending_native_approval_commands(crate::database::now_ms())
    }

    pub(crate) fn recover_native_approvals_detached(&self) -> Result<(), String> {
        let approvals = self.database.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT a.id,t.conversation_id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                 JOIN run_control_bindings b ON b.run_id=t.run_id JOIN kernel_tool_calls n ON n.run_id=t.run_id AND n.tool_call_id=t.runtime_tool_call_id
                 WHERE a.category IN ('tool_execution','task_repair_budget_override') AND a.status IN ('approved','denied') AND a.claimed_at IS NULL
                   AND t.status='pending' AND n.result_json IS NULL AND json_extract(b.binding_json,'$.executorRoute')='pi_native'
                 ORDER BY a.requested_at,a.id")?;
            let rows = query.query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        if approvals.is_empty() { return Ok(()); }
        let host = self.clone();
        std::thread::spawn(move || {
            for (approval, conversation) in approvals {
                let result = (|| -> Result<(), String> {
                    host.ensure_worker(&conversation)?;
                    let stdin = host.state.lock().map_err(|_| "runtime state lock is poisoned")?
                        .worker.as_ref().ok_or("native recovery worker is missing")?.stdin.clone();
                    super::native_tool_batch::recover_native_approval(&host, &stdin, &approval)?;
                    Ok(())
                })();
                if let Err(error) = result { eprintln!("native approval recovery failed for {approval}: {error}"); }
            }
        });
        Ok(())
    }
}
