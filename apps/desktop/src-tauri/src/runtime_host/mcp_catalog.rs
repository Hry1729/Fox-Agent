//! Bounded Run-local schema cache. Cached schemas never authorize execution.
use crate::{database::McpServerRecord, kernel::CancellationToken};
use fox_engine_protocol::RunControlBinding;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::{Mutex, OnceLock}, time::{Duration, Instant}};

const REMOTE_TTL: Duration = Duration::from_secs(300);
const MANAGED_OFFICE_TTL: Duration = Duration::from_secs(24*60*60);
const MAX_ENTRIES: usize = 128;
#[derive(Clone)]
struct Entry { at: Instant, ttl: Duration, checked_at: i64, tools: Vec<Value>, reference: Value }
static CACHE: OnceLock<Mutex<HashMap<String,Entry>>> = OnceLock::new();

fn key(binding: &RunControlBinding, manifest_hash: &str, server: &McpServerRecord) -> String {
    let binary = std::fs::metadata(&server.command).ok().map(|m| (m.len(), m.modified().ok()));
    super::runtime_shadow_hash(&format!("catalog-v1|{}|{}|{}|{}|{:?}|{}", binding.run_id,
        manifest_hash, binding.execution_profile_id, crate::mcp::server_fingerprint(server), binary, server.id))
}

pub(super) fn catalog(binding: &RunControlBinding, manifest_hash: &str, server: &McpServerRecord, token: &CancellationToken,
    budget: Duration, office_reference: bool, load: impl FnOnce() -> Result<Vec<Value>,String>) -> Result<(Vec<Value>,Value),String> {
    token.check()?;
    // Check binary integrity even when the schema is cached: schema discovery
    // is never proof that the executable still satisfies the managed pin.
    if server.id == crate::office::SERVER_ID { crate::office::verify_binary(std::path::Path::new(&server.command))?; }
    let cache = CACHE.get_or_init(||Mutex::new(HashMap::new()));
    let cache_key = key(binding,manifest_hash,server);
    if let Some(entry) = cache.lock().map_err(|_|"MCP catalog cache lock poisoned")?.get(&cache_key).filter(|e|e.at.elapsed()<e.ttl).cloned() {
        return Ok((entry.tools, json!({"catalogVersion":1,"cacheHit":true,"checkedAt":entry.checked_at,
            "schemaAvailability":"discovered","executionAvailability":"unverified","officeReference":entry.reference})));
    }
    let started = Instant::now();
    let tools = load()?;
    token.check()?;
    let reference = if server.id == crate::office::SERVER_ID && office_reference {
        let mut references = Vec::new();
        for element in [None,Some("chart"),Some("range"),Some("cell")] {
            let Some(remaining)=budget.checked_sub(started.elapsed()).filter(|d|!d.is_zero()) else {break};
            token.check()?;
            let mut input=json!({"format":"xlsx"});
            if let Some(element)=element {input["element"]=json!(element);}
            match crate::office::execute_with_cancellation(server,"office_help",&input,
                binding.permission.project_root.as_deref(),binding.permission.mode.as_str(),Some(token),remaining,None) {
                Ok(value)=>{
                    let raw=value["content"].as_array().into_iter().flatten().filter_map(|b|b["text"].as_str()).collect::<Vec<_>>().join("\n");
                    if let Ok(mut value)=serde_json::from_str::<Value>(&raw) {
                        if let Some(object)=value.as_object_mut() {object.retain(|k,_|matches!(k.as_str(),
                            "format"|"element"|"elements"|"properties"|"parent"|"paths"|"totalProperties"|"page"|"pages"|"continuation"|"availableProperties"
                            |"next"|"complete"|"pageSize"|"nextOffset"|"hasMore"));}
                        // Retain current-reference element/property names and
                        // types, not examples or unbounded documentation.
                        if let Some(properties)=value.get_mut("properties").and_then(Value::as_array_mut) {
                            for property in properties.iter_mut() {
                                if let Some(object)=property.as_object_mut() {object.retain(|k,_|matches!(k.as_str(),"name"|"type"|"ops"|"hint"));}
                            }
                        }
                        references.push(json!({"format":"xlsx","element":element,"reference":value}));
                    }
                },
                Err(_)=>references.push(json!({"format":"xlsx","element":element,"availability":"unknown","reasonCode":"office.reference_unavailable"})),
            }
        }
        json!({"source":"current_managed_office_help","executionAvailability":"unverified",
            "references":references,"instruction":"The catalog exposes chart/axis/series schemas; read detailed office_help for selected properties. Discovery does not verify chart write or PDF export availability."})
    } else {Value::Null};
    let checked_at=crate::database::now_ms();
    let mut cache=cache.lock().map_err(|_|"MCP catalog cache lock poisoned")?;
    cache.retain(|_,entry|entry.at.elapsed()<entry.ttl);
    if cache.len()>=MAX_ENTRIES {if let Some(oldest)=cache.iter().min_by_key(|(_,v)|v.at).map(|(k,_)|k.clone()){cache.remove(&oldest);}}
    let ttl=if server.id==crate::office::SERVER_ID {MANAGED_OFFICE_TTL} else {REMOTE_TTL};
    cache.insert(cache_key,Entry{at:Instant::now(),ttl,checked_at,tools:tools.clone(),reference:reference.clone()});
    Ok((tools,json!({"catalogVersion":1,"cacheHit":false,"checkedAt":checked_at,
        "schemaAvailability":"discovered","executionAvailability":"unverified","officeReference":reference})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fox_engine_protocol::{FrozenPermission,PermissionMode,ExecutionAuthority,ResourceExecutor,TimeBudgets};
    #[test]
    fn o14_catalog_reuses_schema_but_invalidates_config_manifest_profile_and_binary_version() {
        let id=uuid::Uuid::new_v4().to_string();
        let binary=std::env::temp_dir().join(format!("fox-catalog-{id}.bin"));
        std::fs::write(&binary,b"version-1").unwrap();
        let mut server=McpServerRecord{id:id.clone(),name:"fixture".into(),command:binary.to_string_lossy().into(),args:vec![],transport:"stdio".into(),endpoint_url:None,
            definition:None,enabled:true,status:"unknown".into(),credential_configured:false,last_error:None,last_checked_at:None,last_latency_ms:None,tool_count:None,consecutive_failures:0,created_at:0,updated_at:0};
        let mut binding=RunControlBinding{schema_version:1,run_id:id.clone(),conversation_id:"c".into(),engine_id:"pi".into(),execution_profile_id:"legacy".into(),
            authority:ExecutionAuthority::Authoritative,read_only_executor:ResourceExecutor::Rust,permission_snapshot_id:"p".into(),
            permission:FrozenPermission{mode:PermissionMode::ReadOnly,project_root:None,grants:vec![],approval_epoch:None},budgets:TimeBudgets::default()};
        let registry=crate::kernel::CancellationRegistry::default();
        registry.register_run(&id).unwrap();
        let token=registry.run_token(&id).unwrap();
        let calls=std::cell::Cell::new(0);
        let read=|binding:&RunControlBinding,hash:&str,server:&McpServerRecord|catalog(binding,hash,server,&token,Duration::from_secs(1),false,||{
            calls.set(calls.get()+1);Ok(vec![json!({"name":"fixture","inputSchema":{"type":"object"}})])}).unwrap();
        assert_eq!(read(&binding,"manifest-1",&server).1["cacheHit"],false);
        let cached=read(&binding,"manifest-1",&server).1;
        assert_eq!(cached["cacheHit"],true);
        assert_eq!(cached["executionAvailability"],"unverified");
        assert_eq!(calls.get(),1);
        server.args.push("new-config".into());read(&binding,"manifest-1",&server);
        read(&binding,"manifest-2",&server);
        binding.execution_profile_id="durable_v2".into();read(&binding,"manifest-2",&server);
        std::fs::write(&binary,b"longer-version-2").unwrap();read(&binding,"manifest-2",&server);
        assert_eq!(calls.get(),5);
        std::fs::remove_file(binary).unwrap();
    }
}

pub(super) fn compact_tools(tools: Vec<Value>) -> Vec<Value> {
    tools.into_iter().map(|mut tool| {
        if let Some(description)=tool["description"].as_str(){tool["description"]=json!(description.chars().take(300).collect::<String>());}
        if let Some(object)=tool.as_object_mut(){object.retain(|k,_|matches!(k.as_str(),"name"|"description"|"inputSchema"|"annotations"));}
        tool
    }).collect()
}
