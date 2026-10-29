use crate::database::Database;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, RunControlBinding, RuntimeEnvelope};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json,Value};
use sha2::{Digest,Sha256};

pub(super) const ADMIT: &str = "model_request.admit";
pub(super) const COMPLETE: &str = "model_request.complete";
const MAX_TOKENS: i64 = 9_007_199_254_740_991;

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(format!("runtime.native_model_budget: {}",message.into()))
}
fn canonical<T: serde::Serialize>(value: &T) -> rusqlite::Result<String> {
    crate::canonical_json::to_string(value).map_err(|error|invalid(error.to_string()))
}
fn hash(value: &str) -> String { format!("sha256:{}",hex::encode(Sha256::digest(value.as_bytes()))) }

pub(super) fn initialize(database: &Database) -> Result<(),String> {
    database.initialize_native_model_budget()
}

#[derive(Clone,Debug,PartialEq,serde::Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Limits {
    per_request_output:i64,total:Option<i64>,output:Option<i64>,duration_ms:Option<i64>,
    runtime_session_id:String,context_generation:i64,
}

fn positive(value:i64) -> rusqlite::Result<i64> {
    if !(1..=MAX_TOKENS).contains(&value) { return Err(invalid("token limits must be positive safe integers")); }
    Ok(value)
}
fn tighten(left:Option<i64>,right:Option<i64>) -> Option<i64> {
    match (left,right) { (Some(left),Some(right))=>Some(left.min(right)),(left,right)=>left.or(right) }
}

fn verify_binding(connection:&Connection,binding:&RunControlBinding) -> rusqlite::Result<String> {
    binding.validate().map_err(invalid)?;
    let (conversation,authority,engine,stored,stored_hash,owner):(String,String,String,String,String,String) = connection.query_row(
        "SELECT b.conversation_id,b.authority,b.engine_id,b.binding_json,b.binding_hash,r.conversation_id
         FROM run_control_bindings b JOIN runs r ON r.id=b.run_id WHERE b.run_id=?1",[&binding.run_id],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
    )?;
    let decoded:RunControlBinding = serde_json::from_str(&stored).map_err(|error|invalid(error.to_string()))?;
    if decoded != *binding || stored_hash != hash(&stored) || binding.permission_snapshot_id != hash(&canonical(&binding.permission)?)
        || binding.conversation_id != conversation || conversation != owner || engine != "pi" || binding.engine_id != engine
        || authority != "authoritative" || binding.authority != ExecutionAuthority::Authoritative
        || binding.executor_route != Some(ExecutorRoute::PiNative) {
        return Err(invalid("model request does not own an intact frozen native Run"));
    }
    Ok(stored_hash)
}

pub(super) fn freeze(database:&Database,binding:&RunControlBinding,per_request_output:i64,
    max_total_tokens:Option<i64>,max_output_tokens:Option<i64>) -> Result<(),String> {
    initialize(database)?;
    database.with_connection(|connection| {
        let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding_hash=verify_binding(&transaction,binding)?;
        let (session,format,generation):(String,i64,i64)=transaction.query_row(
            "SELECT runtime_session_id,format_version,context_generation FROM runtime_sessions WHERE conversation_id=?1 AND runtime_type='pi'",
            [&binding.conversation_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        )?;
        if format<2 || generation<1 { return Err(invalid("model budget requires the current native session mapping")); }
        let mut limits=Limits { per_request_output:positive(per_request_output)?,
            total:max_total_tokens.map(positive).transpose()?,output:max_output_tokens.map(positive).transpose()?,duration_ms:None,
            runtime_session_id:session,context_generation:generation };
        let raw:String=transaction.query_row("SELECT COALESCE(budget_json,'{}') FROM runs WHERE id=?1",[&binding.run_id],|row|row.get(0))?;
        let budget:Value=serde_json::from_str(&raw).map_err(|error|invalid(format!("invalid frozen Run budget: {error}")))?;
        if !budget.is_object() { return Err(invalid("frozen Run budget must be an object")); }
        if let Some(value)=budget.get("maxDurationMs") {
            limits.duration_ms=Some(positive(value.as_i64().ok_or_else(||invalid("invalid frozen duration budget"))?)?);
        }
        for (field,target) in [("maxTotalTokens",&mut limits.total),("maxOutputTokens",&mut limits.output)] {
            if let Some(value)=budget.get(field) {
                let value=positive(value.as_i64().ok_or_else(||invalid("invalid frozen token budget"))?)?;
                *target=tighten(*target,Some(value));
            }
        }
        let child:Option<(i64,i64,i64)>=transaction.query_row(
            "SELECT max_total_tokens,max_output_tokens,max_duration_ms FROM child_run_delegations WHERE child_run_id=?1",[&binding.run_id],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).optional()?;
        if let Some((total,output,duration))=child {
            limits.total=tighten(limits.total,Some(positive(total)?));
            limits.output=tighten(limits.output,Some(positive(output)?));
            limits.duration_ms=tighten(limits.duration_ms,Some(positive(duration)?));
        }
        let encoded=canonical(&limits)?;
        let old:Option<(String,String)>=transaction.query_row(
            "SELECT binding_hash,limits_json FROM native_model_budget_limits WHERE run_id=?1",[&binding.run_id],
            |row|Ok((row.get(0)?,row.get(1)?)),
        ).optional()?;
        if let Some(old)=old {
            if old != (binding_hash,encoded) { return Err(invalid("native model budget is already frozen")); }
        } else {
            transaction.execute("INSERT INTO native_model_budget_limits(run_id,binding_hash,limits_json,created_at) VALUES(?1,?2,?3,?4)",
                params![binding.run_id,binding_hash,encoded,crate::database::now_ms()])?;
        }
        transaction.commit()
    })
}

#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Admission { request_id:String,stage:String,max_output_tokens:i64 }
#[derive(Deserialize,serde::Serialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Completion { request_id:String,input_tokens:Option<i64>,output_tokens:Option<i64>,status:String }

fn identity(connection:&Connection,binding:&RunControlBinding,envelope:&RuntimeEnvelope) -> rusqlite::Result<(Limits,String)> {
    let binding_hash=verify_binding(connection,binding)?;
    if envelope.kind!="request" || envelope.run_id.as_deref()!=Some(binding.run_id.as_str())
        || envelope.conversation_id.as_deref()!=Some(binding.conversation_id.as_str()) {
        return Err(invalid("model request envelope belongs to another Run"));
    }
    let (session,format,generation,run_session):(String,i64,i64,Option<String>)=connection.query_row(
        "SELECT s.runtime_session_id,s.format_version,s.context_generation,r.runtime_session_id
         FROM runtime_sessions s JOIN runs r ON r.conversation_id=s.conversation_id
         WHERE r.id=?1 AND s.runtime_type='pi'",[&binding.run_id],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    )?;
    if format<2 || generation<1 || envelope.runtime_session_id.as_deref()!=Some(session.as_str())
        || run_session.as_deref().is_some_and(|value|value!=session) {
        return Err(invalid("model request session has been retired"));
    }
    let (frozen_hash,raw):(String,String)=connection.query_row(
        "SELECT binding_hash,limits_json FROM native_model_budget_limits WHERE run_id=?1",[&binding.run_id],
        |row|Ok((row.get(0)?,row.get(1)?)),
    )?;
    if frozen_hash!=binding_hash { return Err(invalid("model budget binding changed after freeze")); }
    let limits:Limits=serde_json::from_str(&raw).map_err(|error|invalid(error.to_string()))?;
    if limits.runtime_session_id!=session || limits.context_generation!=generation {
        return Err(invalid("model budget belongs to a retired context generation"));
    }
    positive(limits.per_request_output)?;
    limits.total.map(positive).transpose()?;
    limits.output.map(positive).transpose()?;
    Ok((limits,binding_hash))
}

fn request_id(value:&str) -> rusqlite::Result<()> {
    if value.trim().is_empty() || value.len()>512 { return Err(invalid("invalid model request identity")); }
    Ok(())
}

fn elapsed_at(database:&Database,run:&str,now:i64) -> Result<i64,String> {
    let binding=database.run_control_binding(run)?.ok_or("native execution clock has no frozen binding")?;
    if binding.authority!=ExecutionAuthority::Authoritative || binding.executor_route!=Some(ExecutorRoute::PiNative) || binding.engine_id!="pi" {
        return Err("native execution clock requires the frozen Pi-native route".into());
    }
    let started:Option<i64>=database.with_connection(|connection|connection.query_row("SELECT started_at FROM runs WHERE id=?1",[run],|row|row.get(0)))?;
    let started=started.ok_or("native Run execution clock has not started")?;
    if started>now { return Err("native Run execution clock is in the future".into()); }
    Ok(now.saturating_sub(started).saturating_sub(database.run_approval_wait_ms(run,now)?).max(0))
}

pub(super) fn elapsed_ms(database:&Database,run:&str) -> Result<i64,String> {
    elapsed_at(database,run,crate::database::now_ms())
}

fn spent(connection:&Connection,run:&str,limits:&Limits) -> rusqlite::Result<(i64,i64)> {
    let mut query=connection.prepare("SELECT reserved_total,reserved_output,input_tokens,output_tokens FROM native_model_requests WHERE run_id=?1")?;
    let rows=query.query_map([run],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,Option<i64>>(2)?,row.get::<_,Option<i64>>(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (mut total,mut output)=(0i64,0i64);
    for (reserved_total,reserved_output,input,observed_output) in rows {
        total=total.saturating_add(match (input,observed_output) { (Some(input),Some(output))=>input.saturating_add(output),_=>reserved_total });
        output=output.saturating_add(observed_output.unwrap_or(reserved_output));
    }
    let mut query=connection.prepare("SELECT event_json FROM run_events WHERE run_id=?1 AND event_type='usage.request'
        AND NOT EXISTS(SELECT 1 FROM native_model_requests n WHERE n.run_id=?1 AND n.request_id=json_extract(run_events.event_json,'$.record.requestId'))")?;
    let rows=query.query_map([run],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for raw in rows {
        let event:Value=serde_json::from_str(&raw).map_err(|error|invalid(error.to_string()))?;
        let usage=&event["record"]["usage"];
        let values=["input","output","cacheRead","cacheWrite"].map(|field|usage[field].as_i64());
        if values.iter().all(Option::is_some) {
            total=values.iter().flatten().fold(total,|sum,value|sum.saturating_add(*value));
        } else if let Some(cap)=limits.total { total=total.max(cap); }
        output=output.saturating_add(values[1].unwrap_or(limits.output.unwrap_or(limits.per_request_output)));
    }
    let observed:(i64,i64)=connection.query_row(
        "SELECT COALESCE(MAX(json_extract(event_json,'$.totalTokens')),0),COALESCE(MAX(json_extract(event_json,'$.outputTokens')),0)
         FROM run_events WHERE run_id=?1 AND event_type='usage.updated'",[run],|row|Ok((row.get(0)?,row.get(1)?)),
    )?;
    Ok((total.max(observed.0),output.max(observed.1)))
}

fn denied(code:&str) -> Value {
    json!({"allowed":false,"timeoutMs":0,"firstResponseMs":0,"idleMs":0,"maxOutputTokens":0,"reasonCode":code})
}

fn daily_remaining(connection:&Connection,run:&str) -> rusqlite::Result<Option<i64>> {
    let raw:String=connection.query_row("SELECT COALESCE(budget_json,'{}') FROM runs WHERE id=?1",[run],|row|row.get(0))?;
    let budget:Value=serde_json::from_str(&raw).map_err(|error|invalid(error.to_string()))?;
    if budget["contract"]!="digital_colleague_run_budget_v1" { return Ok(None); }
    let maximum=positive(budget["maxDailyTokens"].as_i64().ok_or_else(||invalid("missing frozen daily token budget"))?)?;
    let start=budget["dailyWindowStartMs"].as_i64().filter(|value|*value>=0).ok_or_else(||invalid("invalid frozen daily window"))?;
    let colleague:String=connection.query_row("SELECT colleague_id FROM digital_colleague_triggers WHERE run_id=?1",[run],|row|row.get(0))?;
    let mut query=connection.prepare("SELECT run_id,total_tokens FROM digital_colleague_triggers
        WHERE colleague_id=?1 AND created_at>=?2 AND status NOT IN ('rejected','skipped')")?;
    let rows=query.query_map(params![colleague,start],|row|Ok((row.get::<_,Option<String>>(0)?,row.get::<_,i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    let mut charged=0i64;
    for (owner,observed) in rows {
        let mut used=observed.max(0);
        if let Some(owner)=owner {
            let frozen:Option<String>=connection.query_row("SELECT limits_json FROM native_model_budget_limits WHERE run_id=?1",[&owner],|row|row.get(0)).optional()?;
            if let Some(frozen)=frozen {
                let limits:Limits=serde_json::from_str(&frozen).map_err(|error|invalid(error.to_string()))?;
                used=used.max(spent(connection,&owner,&limits)?.0);
            }
        }
        charged=charged.saturating_add(used);
    }
    Ok(Some(maximum.saturating_sub(charged)))
}

fn admit_at(database:&Database,envelope:&RuntimeEnvelope,now:i64) -> Result<Value,String> {
    let request:Admission=serde_json::from_value(envelope.payload.clone().ok_or("model admission payload is missing")?).map_err(|error|error.to_string())?;
    let run=envelope.run_id.as_deref().ok_or("model admission Run is missing")?;
    let binding=database.run_control_binding(run)?.ok_or("model admission has no frozen binding")?;
    let elapsed=elapsed_at(database,run,now)?;
    database.with_connection(|connection| {
        let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (limits,_)=identity(&transaction,&binding,envelope)?;
        request_id(&request.request_id)?;
        positive(request.max_output_tokens)?;
        if !["agent","planner","compaction"].contains(&request.stage.as_str()) { return Err(invalid("unknown model request stage")); }
        let state:String=transaction.query_row("SELECT status FROM runs WHERE id=?1",[run],|row|row.get(0))?;
        if state!="running" { return Err(invalid("a new model request requires a running Run")); }
        let old:Option<(String,i64,i64,String,Option<String>,i64)>=transaction.query_row(
            "SELECT stage,requested_output,deadline_at,admission_json,completion_json,admitted_at FROM native_model_requests WHERE run_id=?1 AND request_id=?2",
            params![run,request.request_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        ).optional()?;
        if let Some((stage,requested,deadline,raw,completion,admitted_at))=old {
            if stage!=request.stage || requested!=request.max_output_tokens { return Err(invalid("model admission replay changed its request")); }
            let mut response:Value=serde_json::from_str(&raw).map_err(|error|invalid(error.to_string()))?;
            if response["allowed"]==true {
                if completion.is_some() || deadline<=now { return Ok(denied("runtime.model_request_already_settled")); }
                let remaining=deadline.saturating_sub(now);
                for field in ["timeoutMs","firstResponseMs","idleMs"] {
                    let window=response[field].as_i64().unwrap_or(0).saturating_sub(now.saturating_sub(admitted_at).max(0)).min(remaining);
                    if window<=0 { return Ok(denied("runtime.model_request_already_settled")); }
                    response[field]=json!(window);
                }
            }
            return Ok(response);
        }
        let mut timeout=binding.budgets.limit_operation_ms(binding.budgets.model_request_ms,elapsed);
        if let Some(duration)=limits.duration_ms { timeout=timeout.min(duration.saturating_sub(elapsed)); }
        let (spent_total,spent_output)=spent(&transaction,run,&limits)?;
        let remaining_total=tighten(limits.total.map(|cap|cap.saturating_sub(spent_total).max(0)),daily_remaining(&transaction,run)?);
        let remaining_output=limits.output.map(|cap|cap.saturating_sub(spent_output).max(0));
        let maximum=request.max_output_tokens.min(limits.per_request_output)
            .min(remaining_total.unwrap_or(MAX_TOKENS)).min(remaining_output.unwrap_or(MAX_TOKENS));
        let tools_open:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM kernel_tool_calls WHERE run_id=?1 AND result_json IS NULL)
            OR EXISTS(SELECT 1 FROM tool_calls WHERE run_id=?1 AND status IN ('pending','running'))",[run],|row|row.get(0))?;
        let model_inflight:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM native_model_requests
            WHERE run_id=?1 AND status='admitted' AND completion_json IS NULL)",[run],|row|row.get(0))?;
        let output_exceeded:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM native_model_requests
            WHERE run_id=?1 AND output_tokens>admitted_output)",[run],|row|row.get(0))?;
        let response=if tools_open { denied("runtime.model_request_waiting_tools") }
            else if model_inflight { denied("runtime.model_request_inflight") }
            else if output_exceeded { denied("runtime.model_token_budget_exhausted") }
            else if timeout<=0 { denied("runtime.run_budget_exhausted") }
            else if maximum<=0 { denied("runtime.model_token_budget_exhausted") }
            else { json!({"allowed":true,"timeoutMs":timeout,"firstResponseMs":binding.budgets.model_first_response_ms.min(timeout),
                "idleMs":binding.budgets.model_idle_ms.min(timeout),"maxOutputTokens":maximum}) };
        let allowed=response["allowed"]==true;
        transaction.execute("INSERT INTO native_model_requests(run_id,request_id,stage,requested_output,admitted_output,
            reserved_total,reserved_output,admitted_at,deadline_at,admission_json,status)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![run,request.request_id,request.stage,request.max_output_tokens,if allowed {maximum} else {0},
                if allowed {remaining_total.unwrap_or(0)} else {0},if allowed {maximum} else {0},now,
                now.saturating_add(if allowed {timeout} else {0}),canonical(&response)?,if allowed {"admitted"} else {"denied"}])?;
        transaction.commit()?;
        Ok(response)
    })
}

fn complete(database:&Database,envelope:&RuntimeEnvelope) -> Result<Value,String> {
    let request:Completion=serde_json::from_value(envelope.payload.clone().ok_or("model completion payload is missing")?).map_err(|error|error.to_string())?;
    let run=envelope.run_id.as_deref().ok_or("model completion Run is missing")?;
    let binding=database.run_control_binding(run)?.ok_or("model completion has no frozen binding")?;
    database.with_connection(|connection| {
        let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (limits,_)=identity(&transaction,&binding,envelope)?;
        request_id(&request.request_id)?;
        if !["completed","failed","cancelled"].contains(&request.status.as_str()) { return Err(invalid("unknown model completion status")); }
        if [request.input_tokens,request.output_tokens].into_iter().flatten().any(|value|!(0..=MAX_TOKENS).contains(&value)) {
            return Err(invalid("invalid model usage counters"));
        }
        let raw=canonical(&request)?;
        let old:Option<(Option<String>,String,i64)>=transaction.query_row(
            "SELECT completion_json,status,admitted_output FROM native_model_requests WHERE run_id=?1 AND request_id=?2",
            params![run,request.request_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).optional()?;
        let Some((completion,status,admitted_output))=old else { return Err(invalid("model completion has no Host admission")); };
        if let Some(completion)=completion {
            if completion!=raw { return Err(invalid("model completion is immutable")); }
        } else {
            if status!="admitted" { return Err(invalid("a denied model request cannot complete")); }
            transaction.execute("UPDATE native_model_requests SET completion_json=?3,input_tokens=?4,output_tokens=?5,status=?6
                WHERE run_id=?1 AND request_id=?2 AND completion_json IS NULL",
                params![run,request.request_id,raw,request.input_tokens,request.output_tokens,request.status])?;
        }
        let (total,output)=spent(&transaction,run,&limits)?;
        let exceeded=request.output_tokens.is_some_and(|output|output>admitted_output)
            || limits.total.is_some_and(|cap|total>cap) || limits.output.is_some_and(|cap|output>cap)
            || daily_remaining(&transaction,run)?.is_some_and(|remaining|remaining<0);
        transaction.commit()?;
        if exceeded { return Err(invalid("reported model usage exceeded the frozen token budget")); }
        Ok(json!({"completed":true}))
    })
}

pub(super) fn handles(request:&str) -> bool { matches!(request,ADMIT|COMPLETE) }

impl Database {
    pub(crate) fn enforce_native_model_completion(connection:&Connection,run:&str) -> rusqlite::Result<()> {
        let raw:Option<String>=connection.query_row("SELECT limits_json FROM native_model_budget_limits WHERE run_id=?1",[run],|row|row.get(0)).optional()?;
        let Some(raw)=raw else { return Ok(()); };
        let limits:Limits=serde_json::from_str(&raw).map_err(|error|invalid(error.to_string()))?;
        let inflight:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM native_model_requests WHERE run_id=?1
            AND ((status='admitted' AND completion_json IS NULL) OR output_tokens>admitted_output))",
            [run],|row|row.get(0))?;
        let (total,output)=spent(connection,run,&limits)?;
        if inflight || limits.total.is_some_and(|cap|total>cap) || limits.output.is_some_and(|cap|output>cap)
            || daily_remaining(connection,run)?.is_some_and(|remaining|remaining<0) {
            return Err(invalid("Run completion cannot bypass an unsettled or exceeded model budget"));
        }
        Ok(())
    }
}

pub(super) fn response(database:&Database,envelope:&RuntimeEnvelope) -> Value {
    let result=match envelope.r#type.as_str() {
        ADMIT=>admit_at(database,envelope,crate::database::now_ms()),COMPLETE=>complete(database,envelope),
        _=>Err("unsupported native model budget request".into()),
    };
    result.unwrap_or_else(|error|json!({"isError":true,"error":error,"errorDetails":{"code":"runtime.native_model_budget","retryable":false}}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fox_engine_protocol::{FrozenPermission,PermissionMode,ResourceExecutor,TimeBudgets};

    struct Fixture { database:Database,binding:RunControlBinding,envelope:RuntimeEnvelope,path:std::path::PathBuf }
    impl Fixture {
        fn new(total:Option<i64>,output:Option<i64>) -> Self {
            Self::with_time(total,output,false)
        }
        fn with_time(total:Option<i64>,output:Option<i64>,continuous:bool) -> Self {
            let path=std::env::temp_dir().join(format!("fox-native-model-budget-{}.db",uuid::Uuid::new_v4()));
            let database=Database::open(path.clone()).unwrap();
            let conversation=database.create_conversation(database.default_agent_id(),None,None,Some("ask")).unwrap().id;
            let run=database.create_run(&conversation,"model budget fixture",None).unwrap().run.id;
            let permission=FrozenPermission { mode:PermissionMode::Ask,project_root:None,grants:Vec::new(),approval_epoch:None };
            let binding=RunControlBinding { schema_version:1,run_id:run.clone(),conversation_id:conversation.clone(),engine_id:"pi".into(),
                execution_profile_id:"legacy".into(),authority:ExecutionAuthority::Authoritative,executor_route:Some(ExecutorRoute::PiNative),
                read_only_executor:ResourceExecutor::Rust,permission_snapshot_id:hash(&canonical(&permission).unwrap()),permission,
                budgets:TimeBudgets { model_request_ms:1000,model_first_response_ms:100,model_idle_ms:200,tool_execution_ms:1000,
                    run_execution_ms:2000,run_execution_limited:!continuous,approval_wait_ms:300_000 } };
            database.freeze_run_control(&binding).unwrap();
            let session=format!("native-budget-session-{}",uuid::Uuid::new_v4());
            database.with_connection(|connection| {
                connection.execute("INSERT INTO runtime_sessions(id,conversation_id,runtime_type,runtime_session_id,status,
                    created_at,updated_at,format_version,context_generation,native_session_path)
                    VALUES(?1,?2,'pi',?1,'ready',1,1,2,1,'budget-test-only.jsonl')",params![session,conversation])?;
                connection.execute("UPDATE runs SET status='running',started_at=1000,runtime_session_id=?2 WHERE id=?1",params![run,session])?;
                Ok(())
            }).unwrap();
            freeze(&database,&binding,80,total,output).unwrap();
            let envelope=RuntimeEnvelope { protocol:fox_engine_protocol::PROTOCOL_NAME.into(),version:fox_engine_protocol::PROTOCOL_VERSION,
                kind:"request".into(),id:"model-budget-test".into(),r#type:ADMIT.into(),request_id:None,conversation_id:Some(conversation),
                runtime_session_id:Some(session),run_id:Some(run),seq:None,timestamp:String::new(),payload:None };
            Self { database,binding,envelope,path }
        }
        fn admit(&self,id:&str,stage:&str,now:i64) -> Result<Value,String> {
            let mut envelope=self.envelope.clone();
            envelope.payload=Some(json!({"requestId":id,"stage":stage,"maxOutputTokens":100}));
            admit_at(&self.database,&envelope,now)
        }
        fn complete(&self,id:&str,input:Option<i64>,output:Option<i64>,status:&str) -> Result<Value,String> {
            let mut envelope=self.envelope.clone(); envelope.r#type=COMPLETE.into();
            envelope.payload=Some(json!({"requestId":id,"inputTokens":input,"outputTokens":output,"status":status}));
            complete(&self.database,&envelope)
        }
    }

    #[test]
    #[ignore = "requires an isolated desktop with an in-flight compaction"]
    fn probe_inflight_desktop_budget_and_settlement() {
        let path = std::env::var("FOX_TEST_LIVE_BUDGET_DB").expect("isolated database path");
        let run = std::env::var("FOX_TEST_LIVE_BUDGET_RUN").expect("running budget fixture id");
        let database = Database::open(path.into()).unwrap();
        let binding = database.run_control_binding(&run).unwrap().unwrap();
        let session = database.get_runtime_session(&binding.conversation_id).unwrap().unwrap();
        let (inflight, limits, completed): (String, String, String) = database.with_connection(|connection| connection.query_row(
            "SELECT n.request_id,l.limits_json,(SELECT completion_json FROM native_model_requests WHERE run_id=n.run_id AND status='completed' LIMIT 1)
             FROM native_model_requests n JOIN native_model_budget_limits l ON l.run_id=n.run_id
             WHERE n.run_id=?1 AND n.stage='compaction' AND n.status='admitted'", [&run],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
        let mut envelope = RuntimeEnvelope { protocol:fox_engine_protocol::PROTOCOL_NAME.into(),version:fox_engine_protocol::PROTOCOL_VERSION,
            kind:"request".into(),id:"desktop-overlap-probe".into(),r#type:ADMIT.into(),request_id:None,
            conversation_id:Some(binding.conversation_id),runtime_session_id:Some(session.runtime_session_id),run_id:Some(run.clone()),
            seq:None,timestamp:String::new(),payload:Some(json!({"requestId":"desktop-overlap-probe","stage":"agent","maxOutputTokens":2048})) };
        let denied = admit_at(&database,&envelope,crate::database::now_ms()).unwrap();
        assert_eq!(denied["reasonCode"],"runtime.model_request_inflight");
        assert_eq!(admit_at(&database,&envelope,crate::database::now_ms()).unwrap(),denied);
        envelope.r#type = COMPLETE.into();
        envelope.payload = Some(serde_json::from_str(&completed).unwrap());
        complete(&database,&envelope).unwrap();
        complete(&database,&envelope).unwrap();
        let (count, reserved, after): (i64, i64, String) = database.with_connection(|connection| connection.query_row(
            "SELECT COUNT(*),SUM(reserved_total+reserved_output),(SELECT limits_json FROM native_model_budget_limits WHERE run_id=?1)
             FROM native_model_requests WHERE run_id=?1 AND request_id='desktop-overlap-probe'", [&run],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
        assert_eq!((count,reserved),(1,0));
        assert_eq!(limits,after);
        println!("{}",json!({"runId":run,"inflightRequest":inflight,"overlap":denied,"duplicateSettlement":"idempotent","limitsUnchanged":true}));
    }

    #[test]
    fn initial_retry_and_compaction_requests_share_the_persisted_token_budget() {
        let fixture=Fixture::new(Some(100),Some(90));
        assert_eq!(fixture.admit("initial","agent",1000).unwrap()["maxOutputTokens"],80);
        fixture.complete("initial",Some(10),Some(10),"completed").unwrap();
        assert_eq!(fixture.admit("compact","compaction",1050).unwrap()["maxOutputTokens"],80);
        assert_eq!(fixture.admit("overlap","agent",1050).unwrap()["allowed"],false);
        fixture.complete("compact",Some(10),Some(10),"failed").unwrap();
        assert_eq!(fixture.admit("retry","agent",1100).unwrap()["maxOutputTokens"],60);
    }

    #[test]
    fn missing_usage_retains_the_reservation_after_database_reopen() {
        let fixture=Fixture::new(Some(100),Some(90));
        fixture.admit("unknown","agent",1000).unwrap();
        fixture.complete("unknown",None,Some(5),"failed").unwrap();
        let path=fixture.path.clone(); let envelope=fixture.envelope.clone();
        drop(fixture);
        let database=Database::open(path).unwrap();
        let mut request=envelope; request.payload=Some(json!({"requestId":"retry","stage":"compaction","maxOutputTokens":50}));
        assert_eq!(admit_at(&database,&request,1100).unwrap()["allowed"],false);
        let reserved:i64=database.with_connection(|connection|connection.query_row(
            "SELECT reserved_total FROM native_model_requests WHERE request_id='unknown'",[],|row|row.get(0))).unwrap();
        assert_eq!(reserved,100);
    }

    #[test]
    fn admission_replay_cannot_reset_any_model_time_window_or_token_reservation() {
        let fixture=Fixture::new(Some(100),None);
        assert_eq!(fixture.admit("same","planner",1000).unwrap()["timeoutMs"],1000);
        let replay=fixture.admit("same","planner",1050).unwrap();
        assert_eq!(replay["timeoutMs"],950);
        assert_eq!(replay["firstResponseMs"],50);
        assert_eq!(replay["idleMs"],150);
        assert_eq!(fixture.admit("same","planner",1200).unwrap()["allowed"],false);
        assert!(fixture.admit("same","agent",1050).is_err());
        let count:i64=fixture.database.with_connection(|connection|connection.query_row("SELECT COUNT(*) FROM native_model_requests",[],|row|row.get(0))).unwrap();
        assert_eq!(count,1);
    }

    #[test]
    fn completion_is_idempotent_immutable_and_does_not_allow_a_second_dispatch() {
        let fixture=Fixture::new(Some(100),None);
        assert!(fixture.complete("unadmitted",Some(1),Some(1),"completed").is_err());
        fixture.admit("once","agent",1000).unwrap();
        fixture.complete("once",Some(10),Some(5),"completed").unwrap();
        fixture.complete("once",Some(10),Some(5),"completed").unwrap();
        assert!(fixture.complete("once",Some(10),Some(4),"completed").is_err());
        assert_eq!(fixture.admit("once","agent",1001).unwrap()["allowed"],false);
    }

    #[test]
    fn excessive_actual_usage_is_retained_and_cannot_complete_the_run() {
        let fixture=Fixture::new(Some(100),Some(90));
        fixture.admit("over-budget","agent",1000).unwrap();
        assert!(fixture.database.with_connection(|connection|Database::enforce_native_model_completion(connection,&fixture.binding.run_id)).is_err());
        assert!(fixture.complete("over-budget",Some(90),Some(20),"completed").is_err());
        let recorded:(i64,i64)=fixture.database.with_connection(|connection|connection.query_row(
            "SELECT input_tokens,output_tokens FROM native_model_requests WHERE request_id='over-budget'",[],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
        assert_eq!(recorded,(90,20));
        assert!(fixture.database.with_connection(|connection|Database::enforce_native_model_completion(connection,&fixture.binding.run_id)).is_err());
    }

    #[test]
    fn provider_output_cannot_exceed_the_individual_host_admission() {
        let fixture=Fixture::new(None,None);
        fixture.admit("over-output","agent",1000).unwrap();
        assert!(fixture.complete("over-output",Some(10),Some(81),"completed").is_err());
        assert!(fixture.database.with_connection(|connection|Database::enforce_native_model_completion(connection,&fixture.binding.run_id)).is_err());
        assert_eq!(fixture.admit("after-over-output","agent",1100).unwrap()["allowed"],false);
    }

    #[test]
    fn terminal_cleanup_is_allowed_but_no_new_model_request_can_start() {
        let fixture=Fixture::new(None,None);
        fixture.admit("cancelled","agent",1000).unwrap();
        fixture.database.mark_run_interrupted(&fixture.binding.run_id,"fixture.stop","stopped").unwrap();
        fixture.complete("cancelled",None,None,"cancelled").unwrap();
        assert!(fixture.admit("after","agent",1100).is_err());
    }

    #[test]
    fn host_run_time_budget_is_tightened_by_elapsed_execution() {
        let fixture=Fixture::new(None,None);
        let remaining=fixture.admit("late","agent",2950).unwrap();
        assert_eq!(remaining["timeoutMs"],50);
        assert_eq!(remaining["firstResponseMs"],50);
        assert_eq!(remaining["idleMs"],50);
        assert_eq!(fixture.admit("expired","compaction",3000).unwrap()["allowed"],false);
    }

    #[test]
    fn continuous_runs_keep_finite_model_windows_after_a_day() {
        let fixture=Fixture::with_time(None,None,true);
        let admission=fixture.admit("continuous","agent",86_401_000).unwrap();
        assert_eq!(admission["timeoutMs"],1000);
        assert_eq!(admission["firstResponseMs"],100);
        assert_eq!(admission["idleMs"],200);
    }

    #[test]
    fn approval_wait_is_excluded_from_the_whole_run_execution_clock() {
        let fixture=Fixture::new(None,None);
        let call=fixture.database.create_host_tool_call(&fixture.binding.run_id,"wait-clock","read",&json!({}),"pending",true).unwrap();
        let approval=fixture.database.create_approval(&call.id,"read",&json!({})).unwrap();
        fixture.database.with_connection(|connection| {
            connection.execute("UPDATE approvals SET requested_at=1000,resolved_at=1900,status='denied' WHERE id=?1",[&approval.id])?;
            connection.execute("UPDATE tool_calls SET status='failed',completed_at=1900 WHERE id=?1",[&call.id])?;
            Ok(())
        }).unwrap();
        assert_eq!(fixture.admit("after-wait","agent",2950).unwrap()["timeoutMs"],950);
    }

    #[test]
    fn model_requests_cannot_change_session_binding_or_frozen_budget_limits() {
        let fixture=Fixture::new(Some(100),Some(90));
        assert!(freeze(&fixture.database,&fixture.binding,100,Some(100),Some(90)).is_err());
        let mut envelope=fixture.envelope.clone();
        envelope.runtime_session_id=Some("retired".into());
        envelope.payload=Some(json!({"requestId":"foreign","stage":"agent","maxOutputTokens":50}));
        assert!(admit_at(&fixture.database,&envelope,1000).is_err());
        fixture.database.with_connection(|connection|connection.execute("UPDATE runtime_sessions SET context_generation=context_generation+1 WHERE conversation_id=?1",[&fixture.binding.conversation_id])).unwrap();
        assert!(fixture.admit("retired-generation","agent",1000).is_err());
        fixture.database.with_connection(|connection|connection.execute("UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id=?1",[&fixture.binding.run_id])).unwrap();
        assert!(fixture.admit("corrupt","agent",1000).is_err());
    }

    #[test]
    fn a_missing_frozen_model_budget_never_falls_back_to_unlimited() {
        let fixture=Fixture::new(None,None);
        fixture.database.with_connection(|connection|connection.execute("DELETE FROM native_model_budget_limits WHERE run_id=?1",[&fixture.binding.run_id])).unwrap();
        assert!(fixture.admit("unfrozen","agent",1000).is_err());
    }
}
