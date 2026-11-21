use super::{RuntimeHost, RuntimeToolIngress};
use crate::database::{Database, McpServerRecord};
use fox_engine_protocol::{RunControlBinding, ToolCallExecuteRequest};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

const TOOL: &str = "call_mcp_tool";
const VERIFIED: &str = "runtime.native_mcp_write_verified";

#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrderLookup {
    server_id: String,
    write_tool: String,
    read_tool: String,
}

impl OrderLookup {
    fn configured() -> Option<Self> {
        let value = std::env::var("FOX_NATIVE_MCP_ORDER_RECOVERY").ok()?;
        let rule: Self = serde_json::from_str(&value).ok()?;
        (!rule.server_id.is_empty()
            && !rule.write_tool.is_empty()
            && !rule.read_tool.is_empty()
            && rule.read_tool != rule.write_tool)
            .then_some(rule)
    }
}

#[derive(Deserialize, Serialize)]
struct RecoveryBinding {
    rule: OrderLookup,
    server_fingerprint: u64,
}

pub(super) fn bind_write(
    database: &Database,
    run_id: &str,
    call_id: &str,
    server: &McpServerRecord,
    tool: &str,
    arguments: &Value,
) -> Result<(), String> {
    let Some(rule) = OrderLookup::configured()
        .filter(|rule| rule.server_id == server.id && rule.write_tool == tool)
    else {
        return Ok(());
    };
    if arguments["orderId"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .is_none()
    {
        return Ok(());
    }
    let frozen = serde_json::to_string(&RecoveryBinding {
        rule,
        server_fingerprint: crate::mcp::server_fingerprint(server),
    })
    .map_err(|error| error.to_string())?;
    database.with_connection(|connection| {
        connection.execute(
            "UPDATE kernel_tool_calls SET mcp_recovery_binding_json=?3
             WHERE run_id=?1 AND tool_call_id=?2 AND state='running' AND claimed_at IS NOT NULL
               AND result_json IS NULL AND mcp_recovery_binding_json IS NULL",
            params![run_id, call_id, frozen],
        )?;
        Ok(())
    })
}

pub(super) fn refuse_replay(
    database: &Database,
    binding: &RunControlBinding,
    request: &ToolCallExecuteRequest,
) -> Result<(), String> {
    if request.tool != TOOL {
        return Ok(());
    }
    let (Some(server), Some(tool), Some(order)) = (
        request.input["serverId"].as_str(),
        request.input["tool"].as_str(),
        request.input["arguments"]["orderId"].as_str(),
    ) else {
        return Ok(());
    };
    let blocked: bool = database.with_connection(|connection| {
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM kernel_tool_calls k JOIN runs r ON r.id=k.run_id
              WHERE r.conversation_id=?1 AND r.id<>?2 AND r.status='interrupted'
                AND k.tool='call_mcp_tool' AND k.mcp_recovery_binding_json IS NOT NULL
                AND json_extract(k.canonical_input_json,'$.serverId')=?3
                AND json_extract(k.canonical_input_json,'$.tool')=?4
                AND json_extract(k.canonical_input_json,'$.arguments.orderId')=?5
                AND (k.result_json IS NULL OR json_extract(k.result_json,'$.errorDetails.code')=?6))",
            params![binding.conversation_id, request.run_id, server, tool, order, VERIFIED],
            |row| row.get(0),
        )
    })?;
    if blocked {
        return Err("runtime.native_mcp_replay_blocked: a previous submission for this order lost its receipt; use a read-only query".into());
    }
    Ok(())
}

pub(super) fn reconcile(
    host: &RuntimeHost,
    binding: &RunControlBinding,
    generation: i64,
    call_id: &str,
) -> Result<bool, String> {
    let Some(rule) = OrderLookup::configured() else {
        return Ok(false);
    };
    let candidate = host.database.with_connection(|connection| {
        connection
            .query_row(
                "SELECT k.run_id,k.batch_id,k.canonical_input_json,k.mcp_recovery_binding_json
             FROM kernel_tool_calls k JOIN runs r ON r.id=k.run_id
             WHERE r.conversation_id=?1 AND r.status='interrupted' AND k.context_generation=?2
               AND k.tool_call_id=?3 AND k.tool='call_mcp_tool' AND k.state='running'
               AND k.result_json IS NULL AND k.close_reason IS NULL
               AND k.mcp_recovery_binding_json IS NOT NULL
               AND EXISTS(SELECT 1 FROM runs next WHERE next.id=?4
                          AND next.conversation_id=r.conversation_id
                          AND next.status IN ('queued','running'))",
                params![binding.conversation_id, generation, call_id, binding.run_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
    })?;
    let Some((run_id, batch_id, input, frozen)) = candidate else {
        return Ok(false);
    };
    let frozen: RecoveryBinding =
        serde_json::from_str(&frozen).map_err(|error| error.to_string())?;
    if rule != frozen.rule {
        return Ok(false);
    }
    let Some(server) = host
        .database
        .get_mcp_server(&rule.server_id)?
        .filter(|s| s.enabled)
    else {
        return Ok(false);
    };
    if crate::mcp::server_fingerprint(&server) != frozen.server_fingerprint {
        return Ok(false);
    }
    let input: Value = serde_json::from_str(&input).map_err(|error| error.to_string())?;
    let submitted = &input["arguments"];
    let order_id = submitted["orderId"]
        .as_str()
        .ok_or("Original order ID is absent")?;
    let arguments = json!({"orderId":order_id});
    let query_input =
        json!({"serverId":rule.server_id,"tool":rule.read_tool,"arguments":arguments});
    super::ensure_expert_tool_allowed(&host.database, &binding.conversation_id, TOOL)?;
    host.ensure_delegated_tool_allowed(TOOL)?;
    let (_, scope) =
        super::effective_conversation_delegation_scope(&host.database, &binding.conversation_id)?;
    if [scope.as_ref(), host.mcp_server_scope_override.as_ref()]
        .into_iter()
        .flatten()
        .any(|ids| !ids.contains(&server.id))
    {
        return Ok(false);
    }
    host.ensure_runtime_tool_allowed(
        &binding.run_id,
        TOOL,
        RuntimeToolIngress::Execute,
        Some(&query_input),
    )
    .map_err(|(code, error)| format!("{code}: {error}"))?;
    let hook = crate::lifecycle_hooks::before_tool(
        &host.database,
        &binding.run_id,
        call_id,
        TOOL,
        &query_input,
    )?;
    if hook.requires_approval || hook.blocked.is_some() {
        return Ok(false);
    }
    let budget = binding
        .budgets
        .limit_operation_ms(binding.budgets.tool_execution_ms, 0)
        .min(5_000);
    if budget <= 0 {
        return Ok(false);
    }
    // Session restore precedes registration of the new Run's cancellation scope.
    let cancellation = crate::kernel::CancellationRegistry::default();
    cancellation.register_run(&binding.run_id)?;
    let token = cancellation.run_token(&binding.run_id)?;
    let result = crate::mcp::execute_owned_readonly(
        &server,
        Some((&rule.read_tool, &arguments)),
        &token,
        Duration::from_millis(budget as u64),
    )?;
    let Some(evidence) = verified_order(&result, submitted) else {
        return Ok(false);
    };
    let query_request_id = evidence
        .get("requestId")
        .filter(|id| id.is_u64() || id.as_str().is_some_and(|text| text.len() <= 256));
    token.check()?;
    let response = json!({
        "isError":true,
        "error":format!("The original write did not return a receipt. A read-only query verified exactly one matching order {order_id:?} in the original ledger. Report this verification and do not submit the order again."),
        "errorDetails":{"code":VERIFIED,"retryable":false},
        "nativeMcpRecovery":{
            "source":"connector_read_query","serverId":server.id,"tool":rule.read_tool,
            "originalReceipt":"missing","orderId":order_id,"matchedCount":1,
            "queryRequestId":query_request_id,
            "observedAt":crate::database::now_ms()
        }
    });
    super::native_tool_batch::record_result(
        &host.database,
        &ToolCallExecuteRequest {
            run_id,
            batch_id,
            call_id: call_id.into(),
            tool: TOOL.into(),
            input,
        },
        &response,
    )?;
    Ok(true)
}

fn verified_order<'a>(result: &'a Value, submitted: &Value) -> Option<&'a Value> {
    if result["isError"] != false {
        return None;
    }
    let details = result.get("structuredContent")?;
    let order_id = submitted["orderId"].as_str()?;
    let orders = details["orders"].as_array()?;
    if details["orderId"] != order_id || details["count"] != 1 || orders.len() != 1 {
        return None;
    }
    let order = &orders[0];
    (order["orderId"] == order_id
        && submitted
            .as_object()?
            .iter()
            .all(|(key, value)| order.get(key) == Some(value)))
    .then_some(details)
}

#[cfg(test)]
mod tests {
    use super::super::native_recovery::tests::Fixture;
    use super::*;

    #[test]
    fn only_one_matching_order_is_evidence() {
        let submitted = json!({"orderId":"O900","amount":900});
        let reply = |count, orders| {
            json!({
                "isError":false,
                "structuredContent":{"orderId":"O900","count":count,"orders":orders}
            })
        };
        let matching = json!({"orderId":"O900","amount":900});
        assert!(verified_order(&reply(1, vec![matching.clone()]), &submitted).is_some());
        assert!(verified_order(&reply(0, vec![]), &submitted).is_none());
        assert!(verified_order(&reply(2, vec![matching.clone(), matching]), &submitted).is_none());
        assert!(verified_order(
            &reply(1, vec![json!({"orderId":"O900","amount":901})]),
            &submitted
        )
        .is_none());
        assert!(verified_order(&json!({"isError":true}), &submitted).is_none());
    }

    #[test]
    fn interrupted_order_blocks_replay_but_allows_reads_and_other_orders() {
        let fixture = Fixture::new();
        let input = json!({"serverId":"ledger","tool":"append_order","arguments":{"orderId":"O900","amount":900}});
        fixture.seed(&fixture.old_run, "write", "running", None, true);
        fixture
            .database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE runs SET status='interrupted' WHERE id=?1",
                    [&fixture.old_run],
                )?;
                connection.execute(
                    "UPDATE kernel_tool_calls SET tool='call_mcp_tool',canonical_input_json=?2,
                   mcp_recovery_binding_json='{}' WHERE run_id=?1 AND tool_call_id='write'",
                    params![fixture.old_run, input.to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        let binding = fixture
            .database
            .run_control_binding(&fixture.new_run)
            .unwrap()
            .unwrap();
        let mut request = ToolCallExecuteRequest {
            run_id: fixture.new_run.clone(),
            batch_id: "new-batch".into(),
            call_id: "new-call".into(),
            tool: TOOL.into(),
            input,
        };
        request.input["arguments"]["amount"] = json!(901);
        assert!(refuse_replay(&fixture.database, &binding, &request)
            .unwrap_err()
            .contains("native_mcp_replay_blocked"));
        request.input["tool"] = json!("query_order");
        assert!(refuse_replay(&fixture.database, &binding, &request).is_ok());
        request.input["tool"] = json!("append_order");
        request.input["arguments"]["orderId"] = json!("O901");
        assert!(refuse_replay(&fixture.database, &binding, &request).is_ok());
    }
}
