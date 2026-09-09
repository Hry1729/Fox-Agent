//! User-initiated, bounded read-only queries over the original operation's scope.
use crate::database::{
    Database, McpServerRecord, ReconciliationItem, ReconciliationRequest, ReconciliationView,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

fn hash(value: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes())))
}
fn token() -> crate::kernel::CancellationToken {
    let registry = crate::kernel::CancellationRegistry::default();
    registry
        .register_run("reconciliation-query")
        .expect("valid identity");
    registry
        .run_token("reconciliation-query")
        .expect("registered")
}
fn connection(
    db: &Database,
    request: &ReconciliationRequest,
    item: &ReconciliationItem,
) -> Result<McpServerRecord, String> {
    if item.tool != "call_mcp_tool" {
        return Err("该操作没有可查询的原连接器".into());
    }
    let id = item.input["serverId"].as_str().ok_or("原连接器标识缺失")?;
    let server = db
        .get_mcp_server(id)?
        .filter(|s| s.enabled)
        .ok_or("原连接器已停用或不存在")?;
    let digest=hash(&json!({"id":server.id,"transport":server.transport,"command":server.command,"args":server.args,"endpoint":server.endpoint_url,"definition":server.definition}).to_string());
    if db
        .kernel_host_scope(&request.run_id)?
        .mcp_server_hashes
        .get(id)
        != Some(&digest)
    {
        return Err("原连接器定义已变化，请在外部系统核对后记录依据".into());
    }
    Ok(server)
}

// Inspect the committed operation, never a path supplied by the query request.
fn file_target(item: &ReconciliationItem) -> Option<&str> {
    match item.tool.as_str() {
        "write_file" | "read" | "edit_file" => item.input["path"].as_str(),
        "call_mcp_tool" if item.input["serverId"] == crate::office::SERVER_ID => {
            let key = match item.input["tool"].as_str()? {
                "office_create" | "office_edit" | "office_merge" | "office_render" => "output",
                "office_read" | "office_validate" => "file",
                _ => return None,
            };
            item.input["arguments"][key].as_str()
        }
        _ => None,
    }
}
pub fn options(db: &Database, request: &ReconciliationRequest) -> Result<Value, String> {
    let item = db.kernel_reconciliation_target(request)?;
    if file_target(&item).is_some() {
        if item.tool == "call_mcp_tool" { connection(db, request, &item)?; }
        return Ok(
            json!({"kind":"file","tools":[],"message":"独立读取原目标文件的大小与摘要；一致不代表能确认写入者。"}),
        );
    }
    if item.tool != "call_mcp_tool" {
        return Ok(
            json!({"kind":"manual","tools":[],"message":"此操作没有自动结果查询适配器。请在原系统核对，并填写凭据或核对依据。"}),
        );
    }
    let server = connection(db, request, &item)?;
    let result =
        crate::mcp::execute_owned_readonly(&server, None, &token(), Duration::from_secs(15))?;
    let tools: Vec<Value> = result["tools"]
        .as_array()
        .ok_or("连接器工具列表无效")?
        .iter()
        .filter(|tool| {
            tool["annotations"]["readOnlyHint"] == true
                && tool["annotations"]["destructiveHint"] == false
        })
        .cloned()
        .collect();
    Ok(
        json!({"kind":"connector","tools":tools,"message":"只查询原连接器声明为非破坏性只读的工具。查询返回值仍需人工核对。"}),
    )
}
pub fn query(db: &Database, request: &ReconciliationRequest) -> Result<ReconciliationView, String> {
    let item = db.kernel_reconciliation_target(request)?;
    let observed = crate::database::now_ms();
    let evidence = if let Some(path) = file_target(&item) {
        if item.tool == "call_mcp_tool" { connection(db, request, &item)?; }
        let binding = db
            .run_control_binding(&request.run_id)?
            .ok_or("缺少冻结权限")?;
        match crate::resource_gateway::reconciliation_fingerprint(&binding, path, &token()) {
            Ok(mut result) => {
                result["source"] = json!("independent_file_read");
                result["observedAt"] = json!(observed);
                result["path"] = json!(path);
                result["status"] = json!("observed");
                if item.tool == "write_file" {
                    if let Some(content) = item.input["content"].as_str() {
                        result["matchesSubmittedContent"] =
                            json!(result["sha256"] == hash(content));
                    }
                }
                result["meaning"] =
                    json!("仅证明查询时文件状态，不能独立证明原操作执行过或未执行。");
                result
            }
            Err(_) => {
                json!({"source":"independent_file_read","observedAt":observed,"status":"unavailable","meaning":"文件不可读取、超出权限或大小限制。不能据此判定原操作未执行。"})
            }
        }
    } else {
        let server = connection(db, request, &item)?;
        let tool = request
            .query_tool
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or("请选择只读查询工具")?;
        if !request.arguments.is_object() || request.arguments.to_string().len() > 16_384 {
            return Err("查询条件必须为不超过 16 KiB 的对象".into());
        }
        let result = crate::mcp::execute_owned_readonly(
            &server,
            Some((tool, &request.arguments)),
            &token(),
            Duration::from_secs(15),
        )?;
        let body = result.to_string();
        let excerpt: String = body.chars().take(4000).collect();
        json!({"source":"connector_read_query","serverId":server.id,"tool":tool,"arguments":request.arguments,"observedAt":observed,
            "status":if result["isError"]==true {"query_error"} else {"observed"},"result":excerpt,"truncated":body.chars().count()>4000,
            "resultHash":hash(&body),"meaning":"连接器返回的数据，未自动认定原操作成功；请核对操作标识和业务结果。"})
    };
    db.kernel_record_reconciliation_query(request, &evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconciliation_targets_committed_office_output_and_edit_path_only() {
        let mut item = ReconciliationItem { effect_key: "e".into(), tool: "call_mcp_tool".into(),
            input: json!({"serverId":crate::office::SERVER_ID,"tool":"office_edit","arguments":{"file":"source.docx","output":"result.docx"}}),
            evidence: None, decision: None, note: None };
        assert_eq!(file_target(&item), Some("result.docx"));
        item.input["tool"] = json!("office_read");
        assert_eq!(file_target(&item), Some("source.docx"));
        item.input["serverId"] = json!("foreign");
        assert_eq!(file_target(&item), None);
        item.tool = "edit_file".into();
        item.input = json!({"path":"edited.txt","newText":"replacement"});
        assert_eq!(file_target(&item), Some("edited.txt"));
        item.tool = "execute_command".into();
        assert_eq!(file_target(&item), None, "arbitrary command output cannot be guessed from a path field");
    }
}
