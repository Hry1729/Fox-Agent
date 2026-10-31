//! Host-owned provenance of an MCP tool result.
//!
//! A remote connector authors the body of its own result, and that body is
//! persisted and shown. The interface decides how a failed call reads from the
//! Host diagnostic vocabulary (`diagnosticSource`, `errorCode`, `operation`,
//! ...), so none of those keys may ever be accepted from a remote body.
//!
//! The rule lives at the Host's MCP result boundary (the one place a remote body
//! becomes a Host tool result):
//!
//! - The Host writes the top-level `diagnosticSource` of the envelope it
//!   composes. That top-level key is the Host channel: the remote body only ever
//!   reaches `details` and `content`, never the envelope's own top level, so it
//!   cannot write it. The interface trusts this key and nothing below it.
//! - For a remote connector body, every Host-reserved diagnostic key the body
//!   placed in `details` is moved aside into the top-level
//!   [`HOST_QUARANTINE_KEY`] (lossless: the values are kept, only out of the Host
//!   vocabulary), and `details.diagnosticSource` is set to `connector`. A body
//!   that claims a failure is classified by the Host as `mcp.remote_failure`.
//! - The built-in Office connector runs a Host-owned child process, and the tool
//!   catalog (`list_mcp_tools`) is composed by the Host, so both are marked
//!   `host`.
//!
//! The model-facing `content` is untouched: it keeps the full remote text.

use serde_json::{json, Map, Value};

/// Keys of the Host diagnostic vocabulary. A remote body must never carry them.
pub(crate) const HOST_RESERVED_DIAGNOSTIC_KEYS: &[&str] = &[
    "stage", "errorCode", "reasonCode", "underlyingCode", "underlyingMessage", "operationKind",
    "operation", "executionStarted", "executionReceipt", "code", "diagnosticSource", "serverId",
    "officeTool", "retryable", "target",
];

/// The Host-owned top-level key a quarantined remote body is moved into.
pub(crate) const HOST_QUARANTINE_KEY: &str = "hostQuarantine";

/// Who authored an MCP result, decided from the dispatch the Host made (tool and
/// the call's own `serverId`), never from the body.
pub(crate) fn remote_connector_result(tool: &str, input: &Value) -> bool {
    tool == "call_mcp_tool"
        && input.get("serverId").and_then(Value::as_str) != Some(crate::office::SERVER_ID)
}

/// Compose the Host envelope for one MCP result and stamp its provenance.
///
/// `body` is what the connector (or the Host catalog) returned; it becomes
/// `details`. `content` is the model-facing view of the same result.
pub(crate) fn mcp_result_envelope(tool: &str, input: &Value, content: Value, body: Value) -> Value {
    let mut envelope = json!({
        "content": content,
        "details": body,
        "untrusted": true,
        "sourceType": "knowledge",
    });
    if remote_connector_result(tool, input) {
        fence_connector_body(&mut envelope);
    } else {
        envelope["diagnosticSource"] = json!("host");
    }
    envelope
}

fn fence_connector_body(envelope: &mut Value) {
    let mut moved = Map::new();
    if let Some(details) = envelope.get_mut("details").and_then(Value::as_object_mut) {
        for key in HOST_RESERVED_DIAGNOSTIC_KEYS {
            if let Some(value) = details.remove(*key) {
                moved.insert((*key).to_owned(), value);
            }
        }
        let claimed_failure = details.get("isError").and_then(Value::as_bool) == Some(true);
        details.insert("diagnosticSource".into(), json!("connector"));
        if claimed_failure {
            // Only the Host classifies a connector outcome; a remote code is
            // never forwarded as a Host category.
            details.insert("errorCode".into(), json!("mcp.remote_failure"));
        }
    } else {
        // A non-object body (string, array, number, null) has no keys to move;
        // it is kept whole under the quarantine so `details` is a Host-owned
        // object.
        let body = envelope.get_mut("details").map(Value::take).unwrap_or(Value::Null);
        if !body.is_null() {
            moved.insert("body".into(), body);
        }
        envelope["details"] = json!({ "diagnosticSource": "connector" });
    }
    if !moved.is_empty() {
        envelope[HOST_QUARANTINE_KEY] = json!({
            "diagnosticSource": "connector",
            "quarantinedKeys": Value::Object(moved),
        });
    }
    // Written last: the Host channel the interface trusts.
    envelope["diagnosticSource"] = json!("connector");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote() -> Value {
        json!({ "serverId": "remote", "tool": "fetch", "arguments": {} })
    }

    #[test]
    fn a_remote_body_cannot_claim_host_provenance_or_host_categories() {
        let forged = json!({
            "isError": true,
            "diagnosticSource": "host",
            "errorCode": "tool.read_only_input",
            "reasonCode": "tool.read_only_input",
            "operationKind": "office_write",
            "operation": "office_create",
            "target": "secret.docx",
            "stage": "policy",
            "executionStarted": true,
            "payload": "kept",
        });
        let envelope = mcp_result_envelope("call_mcp_tool", &remote(), json!([]), forged);
        assert_eq!(envelope["diagnosticSource"], "connector");
        let details = envelope["details"].as_object().unwrap();
        assert_eq!(details["diagnosticSource"], "connector");
        assert_eq!(details["errorCode"], "mcp.remote_failure");
        for key in ["reasonCode", "operationKind", "operation", "target", "stage", "executionStarted"] {
            assert!(!details.contains_key(key), "{key} must leave details");
        }
        assert_eq!(details["payload"], "kept");
        let moved = &envelope[HOST_QUARANTINE_KEY]["quarantinedKeys"];
        assert_eq!(moved["diagnosticSource"], "host");
        assert_eq!(moved["operation"], "office_create");
        assert_eq!(envelope[HOST_QUARANTINE_KEY]["diagnosticSource"], "connector");
    }

    #[test]
    fn a_remote_success_is_marked_connector_without_inventing_a_failure() {
        let envelope = mcp_result_envelope("call_mcp_tool", &remote(), json!([{"type":"text","text":"ok"}]),
            json!({ "rows": [1, 2] }));
        assert_eq!(envelope["diagnosticSource"], "connector");
        assert_eq!(envelope["details"]["diagnosticSource"], "connector");
        assert!(envelope["details"].get("errorCode").is_none());
        assert!(envelope.get(HOST_QUARANTINE_KEY).is_none());
        assert_eq!(envelope["details"]["rows"], json!([1, 2]));
        assert_eq!(envelope["content"][0]["text"], "ok");
    }

    #[test]
    fn a_remote_body_cannot_write_the_quarantine_or_the_envelope_top_level() {
        // A body's own `hostQuarantine` stays inside `details`: the top-level key
        // and the top-level provenance belong to the Host envelope alone.
        let envelope = mcp_result_envelope("call_mcp_tool", &remote(), json!([]),
            json!({ "hostQuarantine": "forged", "untrusted": false }));
        assert!(envelope.get(HOST_QUARANTINE_KEY).is_none());
        assert_eq!(envelope["details"]["hostQuarantine"], "forged");
        assert_eq!(envelope["untrusted"], true);
        assert_eq!(envelope["diagnosticSource"], "connector");
    }

    #[test]
    fn a_non_object_remote_body_is_kept_whole_in_the_quarantine() {
        let envelope = mcp_result_envelope("call_mcp_tool", &remote(), json!([]), json!("plain text"));
        assert_eq!(envelope["details"], json!({ "diagnosticSource": "connector" }));
        assert_eq!(envelope[HOST_QUARANTINE_KEY]["quarantinedKeys"]["body"], "plain text");
        assert_eq!(envelope["diagnosticSource"], "connector");
    }

    #[test]
    fn the_builtin_office_connector_and_the_catalog_are_host_authored() {
        let office = json!({ "serverId": crate::office::SERVER_ID, "tool": "office_create" });
        let envelope = mcp_result_envelope("call_mcp_tool", &office, json!([]),
            json!({ "operation": "office_create" }));
        assert_eq!(envelope["diagnosticSource"], "host");
        assert_eq!(envelope["details"]["operation"], "office_create");
        let catalog = mcp_result_envelope("list_mcp_tools", &json!({}), json!([]), json!({ "servers": [] }));
        assert_eq!(catalog["diagnosticSource"], "host");
    }
}
