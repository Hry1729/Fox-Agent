use std::collections::HashSet;
use fox_engine_protocol::{
    NativeToolCallFact, NativeToolCallFactQuery, NativeToolCallFactState,
    RunControlBinding, RuntimeEnvelope, ToolCallFactsRequest, ToolCallFactsResponse,
};
use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use crate::database::Database;

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn inconsistent(message: impl Into<String>) -> rusqlite::Error {
    invalid(format!("runtime.native_recovery_inconsistent: {}", message.into()))
}

fn canonical<T: serde::Serialize>(value: &T) -> rusqlite::Result<String> {
    crate::canonical_json::to_string(value).map_err(|error| invalid(error.to_string()))
}

fn digest(value: &str) -> String { format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes()))) }

pub(super) struct NativeSessionScope {
    pub conversation_id: String,
    pub runtime_session_id: String,
    pub generation: i64,
}

pub(super) fn native_binding(connection: &Connection, run: &str) -> rusqlite::Result<(RunControlBinding, Option<String>)> {
    let (conversation, authority, engine, stored, hash, run_conversation, session):
        (String, String, String, String, String, String, Option<String>) = connection.query_row(
        "SELECT b.conversation_id,b.authority,b.engine_id,b.binding_json,b.binding_hash,
                r.conversation_id,r.runtime_session_id
         FROM run_control_bindings b JOIN runs r ON r.id=b.run_id WHERE b.run_id=?1",
        [run], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
    )?;
    if digest(&stored) != hash { return Err(inconsistent("frozen Run binding hash mismatch")); }
    let binding: RunControlBinding = serde_json::from_str(&stored).map_err(|error| inconsistent(error.to_string()))?;
    binding.validate().map_err(inconsistent)?;
    if binding.run_id != run || binding.conversation_id != conversation || conversation != run_conversation
        || binding.engine_id != engine || authority != "authoritative"
        || binding.permission_snapshot_id != digest(&canonical(&binding.permission)?)
        || !matches!(super::kernel_authority::resolve_run_route(Some(&binding)), super::kernel_authority::RunRoute::PiNative) {
        return Err(inconsistent("Run binding does not own this native session"));
    }
    Ok((binding, session))
}

pub(super) fn session_scope(connection: &Connection, envelope: &RuntimeEnvelope,
    generation: Option<i64>) -> rusqlite::Result<NativeSessionScope> {
    let run = envelope.run_id.as_deref().filter(|run| !run.trim().is_empty())
        .ok_or_else(|| invalid("runtime.native_recovery_identity: missing Run identity"))?;
    let (binding, run_session) = native_binding(connection, run)?;
    if envelope.kind != "request" || envelope.conversation_id.as_deref() != Some(binding.conversation_id.as_str())
        || envelope.payload.as_ref().and_then(|payload| payload["runId"].as_str()) != Some(run) {
        return Err(invalid("runtime.native_recovery_identity: envelope does not match its frozen Run"));
    }
    let (session, actual_generation, format): (String, i64, i64) = connection.query_row(
        "SELECT runtime_session_id,context_generation,format_version FROM runtime_sessions
         WHERE conversation_id=?1 AND runtime_type='pi'", [&binding.conversation_id],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    )?;
    if format < 2 || actual_generation < 1 || envelope.runtime_session_id.as_deref() != Some(session.as_str())
        || generation.is_some_and(|requested| requested != actual_generation)
        || run_session.as_deref().is_some_and(|recorded| recorded != session) {
        return Err(invalid("runtime.native_recovery_identity: session or context generation is retired"));
    }
    Ok(NativeSessionScope { conversation_id:binding.conversation_id, runtime_session_id:session, generation:actual_generation })
}

struct StoredCall {
    run_id: String,
    batch_id: String,
    tool: String,
    input_digest: Option<String>,
    input: String,
    state: String,
    result: Option<String>,
    claimed_at: Option<i64>,
}

fn stored_call(connection: &Connection, scope: &NativeSessionScope,
    call: &NativeToolCallFactQuery) -> rusqlite::Result<Option<StoredCall>> {
    let mut query = connection.prepare(
        "SELECT k.run_id,k.batch_id,k.tool,k.input_digest,k.canonical_input_json,k.state,k.result_json,k.claimed_at
         FROM kernel_tool_calls k JOIN runs r ON r.id=k.run_id
         LEFT JOIN run_control_bindings b ON b.run_id=k.run_id
         WHERE (r.conversation_id=?1 OR b.conversation_id=?1) AND k.context_generation=?2
           AND k.tool_call_id=?3 AND (k.input_digest IS NOT NULL OR json_extract(b.binding_json,'$.executorRoute')='pi_native')",
    )?;
    let rows = query.query_map(params![scope.conversation_id,scope.generation,call.call_id], |row| Ok(StoredCall {
        run_id:row.get(0)?,batch_id:row.get(1)?,tool:row.get(2)?,input_digest:row.get(3)?,
        input:row.get(4)?,state:row.get(5)?,result:row.get(6)?,claimed_at:row.get(7)?,
    }))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 1 { return Err(inconsistent("call id belongs to several Runs in the same context generation")); }
    let Some(row) = rows.into_iter().next() else {
        let retired: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM kernel_tool_calls k JOIN runs r ON r.id=k.run_id
             LEFT JOIN run_control_bindings b ON b.run_id=k.run_id WHERE (r.conversation_id=?1 OR b.conversation_id=?1)
               AND k.context_generation<>?2 AND k.tool_call_id=?3
               AND (k.input_digest IS NOT NULL OR json_extract(b.binding_json,'$.executorRoute')='pi_native'))",
            params![scope.conversation_id,scope.generation,call.call_id], |row| row.get(0),
        )?;
        if retired { return Err(inconsistent("active branch contains a call from a retired context generation")); }
        return Ok(None);
    };
    let (binding, session) = native_binding(connection, &row.run_id)?;
    if binding.conversation_id != scope.conversation_id
        || session.as_deref().is_some_and(|recorded| recorded != scope.runtime_session_id)
        || row.tool != call.tool || row.input_digest.as_deref() != Some(call.input_digest.as_str()) {
        return Err(inconsistent("active branch call disagrees with its Host identity or arguments"));
    }
    Ok(Some(row))
}

fn waiting_approval(connection: &Connection, row: &StoredCall, call_id: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM tool_calls t JOIN approvals a ON a.tool_call_id=t.id
         WHERE t.run_id=?1 AND t.runtime_tool_call_id=?2 AND t.execution_location='host'
           AND t.tool_name=?3 AND t.input_json=?4 AND t.status='pending' AND t.requires_approval=1
           AND a.category IN ('tool_execution','task_repair_budget_override')
           AND a.status IN ('pending','approved','denied') AND a.claimed_at IS NULL)",
        params![row.run_id,call_id,row.tool,row.input], |query| query.get(0),
    )
}

fn nonexecution_response() -> Value {
    json!({"isError":true,"error":"Tool was not executed before the native session stopped.",
        "errorDetails":{"code":"runtime.native_tool_not_executed","retryable":false}})
}

fn validate_calls(request: &ToolCallFactsRequest) -> Result<(), String> {
    if request.run_id.trim().is_empty() || request.context_generation < 1 {
        return Err("runtime.native_recovery_identity: a Run and positive generation are required".into());
    }
    let mut ids = HashSet::new();
    for call in &request.calls {
        if call.call_id.trim().is_empty() || call.tool.trim().is_empty() || !ids.insert(&call.call_id)
            || !call.input_digest.strip_prefix("sha256:")
                .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            return Err("runtime.native_recovery_inconsistent: invalid or duplicate active branch call".into());
        }
    }
    Ok(())
}

fn hydrate_host_receipts(database: &Database, envelope: &RuntimeEnvelope,
    request: &ToolCallFactsRequest) -> Result<(), String> {
    let rows = database.with_connection(|connection| {
        let tx = connection.transaction()?;
        let scope = session_scope(&tx, envelope, Some(request.context_generation))?;
        request.calls.iter().map(|call| stored_call(&tx, &scope, call)).collect::<rusqlite::Result<Vec<_>>>()
    })?;
    let mut receipts = Vec::new();
    for (call, row) in request.calls.iter().zip(rows) {
        if row.as_ref().is_some_and(|row| row.result.is_some()) { continue; }
        let committed = if let Some(row) = row {
            let record = database.get_runtime_tool_call(&row.run_id, &call.call_id)?
                .filter(|record| record.execution_location == "host");
            if let Some(record) = record.as_ref() {
                if record.tool_name != row.tool
                    || canonical(&record.input).map_err(|error| error.to_string())? != row.input {
                    return Err("runtime.native_recovery_inconsistent: Host execution input disagrees with native claim".into());
                }
            }
            let denied = if record.as_ref().is_some_and(|record|record.status=="failed" && record.result.is_none()) {
                database.with_connection(|connection|connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                     WHERE t.run_id=?1 AND t.runtime_tool_call_id=?2 AND t.requires_approval=1
                       AND a.status='denied' AND a.claimed_at IS NULL)",params![row.run_id,call.call_id],|query|query.get::<_,bool>(0),
                ))?
            } else { false };
            match record.filter(|record| matches!(record.status.as_str(), "completed" | "failed") && (record.result.is_some() || denied)) {
                Some(record) => {
                    if row.state != "running" || row.claimed_at.is_none() {
                        return Err("runtime.native_recovery_inconsistent: committed Host execution has no native claim".into());
                    }
                    Some((row, record))
                }
                None => None,
            }
        } else { None };
        if call.has_result && committed.is_none() {
            return Err("runtime.native_recovery_inconsistent: Pi contains a result without a committed Host receipt".into());
        }
        if let Some((row, record)) = committed { receipts.push((call.call_id.clone(), row, record)); }
    }
    for (call_id, row, record) in receipts {
        let response = match record.result.clone() {
            Some(result) => super::settled_host_tool_call_envelope(database, &row.run_id, &call_id,
                &record.conversation_id, result,
                (record.status == "failed").then(|| record.error_message.clone().unwrap_or_else(|| "The persisted tool execution failed.".into())),
                Vec::new(), false),
            None => json!({"isError":true,"error":record.error_message.as_deref().unwrap_or("The user denied this operation.")}),
        };
        let input = serde_json::from_str(&row.input).map_err(|error| format!("runtime.native_recovery_inconsistent: {error}"))?;
        super::native_tool_batch::record_result(database, &fox_engine_protocol::ToolCallExecuteRequest {
            run_id:row.run_id,batch_id:row.batch_id,call_id,tool:row.tool,input,
        }, &response)?;
    }
    Ok(())
}

fn query(database: &Database, envelope: &RuntimeEnvelope) -> Result<ToolCallFactsResponse, String> {
    let request: ToolCallFactsRequest = serde_json::from_value(envelope.payload.clone().ok_or("native facts omitted payload")?)
        .map_err(|error| format!("runtime.native_recovery_identity: {error}"))?;
    validate_calls(&request)?;
    hydrate_host_receipts(database, envelope, &request)?;
    database.with_connection(|connection| {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope = session_scope(&tx, envelope, Some(request.context_generation))?;
        let mut facts = Vec::with_capacity(request.calls.len());
        let mut recovery_batch: Option<String> = None;
        let mut recovered_calls = Vec::new();
        for call in &request.calls {
            let stored = stored_call(&tx, &scope, call)?;
            if call.has_result && stored.as_ref().and_then(|row| row.result.as_ref()).is_none() {
                return Err(inconsistent("Pi contains a result without a committed Host receipt"));
            }
            let (state, response) = if let Some(row) = stored {
                if let Some(result) = &row.result {
                    let response: Value = serde_json::from_str(result).map_err(|error| inconsistent(error.to_string()))?;
                    if !response.is_object() || !matches!(row.state.as_str(),"completed" | "failed" | "cancelled" | "not_started")
                        || (response["isError"] != true && response.get("result").is_none()) {
                        return Err(inconsistent("committed Host receipt has an invalid terminal state or response"));
                    }
                    (if row.state == "not_started" { NativeToolCallFactState::NotExecuted }
                        else { NativeToolCallFactState::Committed }, Some(response))
                } else if waiting_approval(&tx, &row, &call.call_id)? {
                    (NativeToolCallFactState::WaitingApproval, None)
                } else if row.state == "pending" && row.claimed_at.is_none() {
                    let response = nonexecution_response();
                    let changed = tx.execute(
                        "UPDATE kernel_tool_calls SET state='not_started',close_reason='not_started',result_json=?4,settled_at=?5
                         WHERE run_id=?1 AND batch_id=?2 AND tool_call_id=?3 AND state='pending'
                           AND claimed_at IS NULL AND result_json IS NULL",
                        params![row.run_id,row.batch_id,call.call_id,canonical(&response)?,crate::database::now_ms()],
                    )?;
                    if changed != 1 { return Err(inconsistent("nonexecution settlement lost its pending position")); }
                    (NativeToolCallFactState::NotExecuted, Some(response))
                } else if row.claimed_at.is_some() || row.state == "running" {
                    (NativeToolCallFactState::OutcomeUnknown, None)
                } else { return Err(inconsistent("unsettled Host call has no execution or nonexecution proof")); }
            } else {
                // Without registration the managed executor could not have run.
                // Persist that conclusion before Pi receives an appendable result.
                let batch = recovery_batch.get_or_insert_with(|| format!("native-recovery-batch-{}",uuid::Uuid::new_v4()));
                if recovered_calls.is_empty() {
                    tx.execute("INSERT INTO kernel_tool_batches(batch_id,run_id,ordered_tool_call_ids_json,created_at)
                                VALUES(?1,?2,'[]',?3)", params![batch.as_str(),request.run_id,crate::database::now_ms()])?;
                }
                let response = nonexecution_response();
                tx.execute("INSERT INTO kernel_tool_calls(run_id,tool_call_id,batch_id,tool,source_order,
                            canonical_input_json,state,result_json,created_at,settled_at,context_generation,input_digest,close_reason)
                            VALUES(?1,?2,?3,?4,?5,'{}','not_started',?6,?7,?7,?8,?9,'not_started')",
                    params![request.run_id,call.call_id,batch.as_str(),call.tool,recovered_calls.len() as i64,
                        canonical(&response)?,crate::database::now_ms(),scope.generation,call.input_digest])?;
                recovered_calls.push(call.call_id.clone());
                (NativeToolCallFactState::NotExecuted, Some(response))
            };
            facts.push(NativeToolCallFact { call_id:call.call_id.clone(),state,response });
        }
        if let Some(batch) = recovery_batch {
            tx.execute("UPDATE kernel_tool_batches SET ordered_tool_call_ids_json=?2 WHERE batch_id=?1",
                params![batch,canonical(&recovered_calls)?])?;
        }
        let blocked = facts.iter().any(|fact| matches!(fact.state,
            NativeToolCallFactState::OutcomeUnknown | NativeToolCallFactState::WaitingApproval));
        tx.commit()?;
        Ok(ToolCallFactsResponse { context_generation:scope.generation,blocked,facts })
    })
}

pub(super) fn response(database: &Database, envelope: &RuntimeEnvelope) -> Value {
    query(database, envelope).and_then(|response| serde_json::to_value(response).map_err(|error| error.to_string()))
        .unwrap_or_else(|error| {
            let code = if error.contains("runtime.native_recovery_inconsistent") {
                "runtime.native_recovery_inconsistent"
            } else { "runtime.native_recovery_identity" };
            json!({"isError":true,"error":error,"errorDetails":{"code":code,"retryable":false}})
        })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, FrozenPermission, PermissionMode, ResourceExecutor, TimeBudgets};

    pub(crate) struct Fixture {
        pub database: Database,
        pub conversation: String,
        pub old_run: String,
        pub new_run: String,
        pub session: String,
    }

    impl Fixture {
        pub(crate) fn new() -> Self {
            Self::with_root(None)
        }

        pub(crate) fn with_root(root: Option<&str>) -> Self {
            let database = Database::open(":memory:".into()).unwrap();
            let conversation = database.create_conversation(database.default_agent_id(), Some("native recovery"), root, Some("ask")).unwrap().id;
            let old_run = database.create_run(&conversation, "old native prompt", None).unwrap().run.id;
            database.with_connection(|connection| connection.execute(
                "UPDATE runs SET status='completed',finished_at=?2 WHERE id=?1",params![old_run,crate::database::now_ms()],
            )).unwrap();
            let new_run = database.create_run(&conversation, "next native prompt", None).unwrap().run.id;
            let permission = FrozenPermission { mode:PermissionMode::Ask,project_root:root.map(str::to_owned),grants:Vec::new(),approval_epoch:None };
            for run in [&old_run,&new_run] {
                database.freeze_run_control(&RunControlBinding {
                    schema_version:fox_engine_protocol::CONTROL_SCHEMA_VERSION,run_id:run.clone(),conversation_id:conversation.clone(),
                    engine_id:"pi".into(),execution_profile_id:"legacy".into(),authority:ExecutionAuthority::Authoritative,
                    executor_route:Some(ExecutorRoute::PiNative),read_only_executor:ResourceExecutor::Rust,
                    permission_snapshot_id:digest(&canonical(&permission).unwrap()),permission:permission.clone(),budgets:TimeBudgets::default(),
                }).unwrap();
            }
            let session = format!("native-test-session-{}",uuid::Uuid::new_v4());
            database.with_connection(|connection| {
                connection.execute("INSERT INTO runtime_sessions(id,conversation_id,runtime_type,runtime_session_id,status,
                                    created_at,updated_at,format_version,context_generation,native_session_path)
                                    VALUES(?1,?2,'pi',?1,'ready',1,1,2,3,'test-only-native-session.jsonl')",
                    params![session,conversation])?;
                connection.execute("UPDATE runs SET runtime_session_id=?2 WHERE conversation_id=?1",params![conversation,session])?;
                connection.execute("UPDATE runs SET status='running',started_at=?2 WHERE id=?1",params![new_run,crate::database::now_ms()])?;
                Ok(())
            }).unwrap();
            Self { database,conversation,old_run,new_run,session }
        }

        pub(crate) fn activate_old(&self) {
            self.database.with_connection(|connection| {
                connection.execute("UPDATE runs SET status='queued',started_at=NULL WHERE id=?1",[&self.new_run])?;
                connection.execute("UPDATE runs SET status='running',finished_at=NULL,started_at=?2 WHERE id=?1",
                    params![self.old_run,crate::database::now_ms()])?;
                Ok(())
            }).unwrap();
        }

        pub(crate) fn call(id: &str) -> NativeToolCallFactQuery {
            NativeToolCallFactQuery { call_id:id.into(),tool:"read".into(),input_digest:format!("sha256:{}","a".repeat(64)),has_result:false }
        }

        pub(crate) fn envelope(&self, calls: Vec<NativeToolCallFactQuery>) -> RuntimeEnvelope {
            RuntimeEnvelope { protocol:fox_engine_protocol::PROTOCOL_NAME.into(),version:fox_engine_protocol::PROTOCOL_VERSION,
                kind:"request".into(),id:"test-facts".into(),r#type:fox_engine_protocol::TOOL_CALL_FACTS.into(),request_id:None,
                conversation_id:Some(self.conversation.clone()),runtime_session_id:Some(self.session.clone()),run_id:Some(self.new_run.clone()),
                seq:None,timestamp:String::new(),payload:Some(serde_json::to_value(ToolCallFactsRequest {
                    run_id:self.new_run.clone(),context_generation:3,calls,
                }).unwrap()) }
        }

        pub(crate) fn seed(&self, run: &str, call: &str, state: &str, result: Option<Value>, claimed: bool) -> String {
            let batch = format!("native-batch-{}",uuid::Uuid::new_v4());
            self.database.with_connection(|connection| {
                connection.execute("INSERT INTO kernel_tool_batches(batch_id,run_id,ordered_tool_call_ids_json,created_at)
                                    VALUES(?1,?2,?3,1)", params![batch,run,canonical(&vec![call])?])?;
                connection.execute("INSERT INTO kernel_tool_calls(run_id,tool_call_id,batch_id,tool,source_order,canonical_input_json,
                                    state,result_json,created_at,context_generation,input_digest,claimed_at)
                                    VALUES(?1,?2,?3,'read',0,'{}',?4,?5,1,3,?6,?7)",
                    params![run,call,batch,state,result.map(|value| canonical(&value)).transpose()?,
                        Self::call(call).input_digest,claimed.then_some(10_i64)])?;
                Ok(())
            }).unwrap();
            batch
        }

        pub(crate) fn host_call(&self, call: &str, status: &str, result: Option<Value>) -> String {
            let id = format!("host-call-{}",uuid::Uuid::new_v4());
            self.database.with_connection(|connection| {
                connection.execute("INSERT INTO tool_calls(id,runtime_tool_call_id,run_id,conversation_id,tool_name,input_json,status,
                                    result_json,execution_location,requires_approval,started_at,updated_at)
                                    VALUES(?1,?2,?3,?4,'read','{}',?5,?6,'host',1,1,1)",
                    params![id,call,self.old_run,self.conversation,status,result.map(|value| canonical(&value)).transpose()?])?;
                Ok(())
            }).unwrap();
            id
        }

        pub(crate) fn approval(&self, host_call: &str, status: &str) -> String {
            let id = format!("approval-{}",uuid::Uuid::new_v4());
            self.database.with_connection(|connection| {
                connection.execute("INSERT INTO approvals(id,tool_call_id,status,requested_action,request_json,requested_at,category,decision_json)
                                    VALUES(?1,?2,?3,'test only','{}',1,'tool_execution',?4)",
                    params![id,host_call,status,canonical(&json!({"approved":status == "approved","scope":"once","category":"tool_execution"}))?])?;
                Ok(())
            }).unwrap();
            id
        }
    }

    #[test]
    fn old_run_receipts_and_new_unexecuted_calls_reconcile_in_one_session() {
        let fixture = Fixture::new();
        let original = json!({"isError":true,"error":"original diagnostics","result":{"content":[{"type":"text","text":"retained"}]},
            "toolResultStorage":{"stored":true},"resultRef":"fox-result://old/call"});
        fixture.seed(&fixture.old_run,"old","failed",Some(original.clone()),true);
        fixture.seed(&fixture.new_run,"pending","pending",None,false);
        fixture.database.with_connection(|connection| connection.execute("UPDATE runs SET status='completed' WHERE id=?1",[&fixture.old_run])).unwrap();
        let calls = vec![Fixture::call("old"),Fixture::call("pending"),Fixture::call("orphan")];
        let result = query(&fixture.database,&fixture.envelope(calls.clone())).unwrap();
        assert!(!result.blocked);
        assert_eq!(result.facts[0].response,Some(original));
        assert_eq!(result.facts[1].state,NativeToolCallFactState::NotExecuted);
        assert_eq!(result.facts[2].state,NativeToolCallFactState::NotExecuted);
        let repeat = query(&fixture.database,&fixture.envelope(calls.into_iter().map(|mut call| {call.has_result=true;call}).collect())).unwrap();
        assert_eq!(repeat,result,"a subsequent Pi result must still have the durable Host nonexecution receipt");
        fixture.database.with_connection(|connection| {
            let state: String = connection.query_row("SELECT state FROM kernel_tool_calls WHERE tool_call_id='pending'",[],|row|row.get(0))?;
            assert_eq!(state,"not_started");
            Ok(())
        }).unwrap();
    }

    #[test]
    fn pi_only_results_abort_before_any_partial_nonexecution_write() {
        let fixture = Fixture::new();
        let mut pi_only = Fixture::call("unrecorded-result"); pi_only.has_result=true;
        let envelope = fixture.envelope(vec![Fixture::call("unrecorded-orphan"),pi_only]);
        assert!(query(&fixture.database,&envelope).unwrap_err().contains("without a committed Host receipt"));
        let body = response(&fixture.database,&envelope);
        assert_eq!(body["errorDetails"]["code"],"runtime.native_recovery_inconsistent");
        fixture.database.with_connection(|connection| {
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM kernel_tool_calls",[],|row|row.get::<_,i64>(0))?,0);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn unknown_outcomes_and_unclaimed_approvals_block_without_appendable_results() {
        let fixture = Fixture::new();
        fixture.activate_old();
        fixture.seed(&fixture.old_run,"unknown","running",None,true);
        fixture.seed(&fixture.old_run,"approval","running",None,true);
        let host_call = fixture.host_call("approval","pending",None);
        let approval = fixture.approval(&host_call,"pending");
        let envelope = fixture.envelope(vec![Fixture::call("unknown"),Fixture::call("approval")]);
        for approved in [false,true] {
            if approved { fixture.database.with_connection(|connection| connection.execute("UPDATE approvals SET status='approved' WHERE id=?1",[&approval])).unwrap(); }
            let result = query(&fixture.database,&envelope).unwrap();
            assert!(result.blocked);
            assert_eq!(result.facts[0].state,NativeToolCallFactState::OutcomeUnknown);
            assert_eq!(result.facts[1].state,NativeToolCallFactState::WaitingApproval);
            assert!(result.facts.iter().all(|fact| fact.response.is_none()));
        }
        fixture.database.with_connection(|connection| connection.execute("UPDATE approvals SET claimed_at=11 WHERE id=?1",[&approval])).unwrap();
        assert_eq!(query(&fixture.database,&envelope).unwrap().facts[1].state,NativeToolCallFactState::OutcomeUnknown);
    }

    #[test]
    fn tool_commit_before_native_commit_recovers_the_persisted_diagnostics() {
        let fixture = Fixture::new();
        fixture.seed(&fixture.old_run,"completed-effect","running",None,true);
        fixture.host_call("completed-effect","failed",Some(json!({"content":[{"type":"text","text":"effect complete; retained diagnostics"}],
            "details":{"errorCode":"tool.execution_failed"}})));
        let mut call = Fixture::call("completed-effect"); call.has_result=true;
        let result = query(&fixture.database,&fixture.envelope(vec![call])).unwrap();
        assert!(!result.blocked);
        let response = result.facts[0].response.as_ref().unwrap();
        assert_eq!(response["isError"],true);
        assert_eq!(response["result"]["content"][0]["text"],"effect complete; retained diagnostics");
        let repeated = query(&fixture.database,&fixture.envelope(vec![Fixture::call("completed-effect")])).unwrap();
        assert_eq!(repeated,result);
    }

    #[test]
    fn a_startup_failed_status_without_a_real_result_stays_unknown() {
        let fixture = Fixture::new();
        fixture.seed(&fixture.old_run,"interrupted-effect","running",None,true);
        fixture.host_call("interrupted-effect","failed",None);
        let result = query(&fixture.database,&fixture.envelope(vec![Fixture::call("interrupted-effect")])).unwrap();
        assert!(result.blocked);
        assert_eq!(result.facts[0].state,NativeToolCallFactState::OutcomeUnknown);
        assert!(result.facts[0].response.is_none());
    }

    #[test]
    fn a_committed_denial_without_tool_output_is_not_an_unknown_effect() {
        let fixture=Fixture::new();
        fixture.seed(&fixture.old_run,"denied","running",None,true);
        let host=fixture.host_call("denied","failed",None);
        fixture.approval(&host,"denied");
        let result=query(&fixture.database,&fixture.envelope(vec![Fixture::call("denied")])).unwrap();
        assert!(!result.blocked);
        assert_eq!(result.facts[0].state,NativeToolCallFactState::Committed);
        assert_eq!(result.facts[0].response.as_ref().unwrap()["isError"],true);
    }

    #[test]
    fn identity_generation_arguments_and_ambiguous_call_ownership_fail_closed() {
        let fixture = Fixture::new();
        fixture.seed(&fixture.old_run,"old","completed",Some(json!({"isError":false,"result":"old"})),true);
        let envelope = fixture.envelope(vec![Fixture::call("old")]);
        let mut changed = envelope.clone(); changed.conversation_id=Some("foreign".into());
        assert!(query(&fixture.database,&changed).is_err());
        changed=envelope.clone(); changed.runtime_session_id=Some("retired".into());
        assert!(query(&fixture.database,&changed).is_err());
        changed=envelope.clone(); changed.payload.as_mut().unwrap()["contextGeneration"]=json!(2);
        assert!(query(&fixture.database,&changed).is_err());
        changed=envelope.clone(); changed.payload.as_mut().unwrap()["calls"][0]["inputDigest"]=json!(format!("sha256:{}","b".repeat(64)));
        assert!(query(&fixture.database,&changed).is_err());
        fixture.seed(&fixture.new_run,"old","completed",Some(json!({"isError":false,"result":"different Run"})),true);
        assert!(query(&fixture.database,&envelope).unwrap_err().contains("several Runs"));
    }

    #[test]
    fn retired_branch_calls_and_corrupt_frozen_bindings_are_never_reclassified_as_unexecuted() {
        let fixture = Fixture::new();
        fixture.seed(&fixture.old_run,"retired","running",None,true);
        fixture.database.with_connection(|connection| connection.execute("UPDATE kernel_tool_calls SET context_generation=2 WHERE tool_call_id='retired'",[])).unwrap();
        assert!(query(&fixture.database,&fixture.envelope(vec![Fixture::call("retired")])).unwrap_err().contains("retired context"));
        fixture.database.with_connection(|connection| connection.execute("UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id=?1",[&fixture.new_run])).unwrap();
        assert!(query(&fixture.database,&fixture.envelope(vec![Fixture::call("new")])).unwrap_err().contains("hash mismatch"));
    }
}
