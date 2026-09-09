//! A bounded, Host-authored projection for model reporting. It does not alter
//! stored tool output, reread resources, grant permission, or replay effects.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) fn append_execution_receipt(
    result: &mut Value,
    run_id: &str,
    tool_id: &str,
    tool: &str,
    completed: bool,
    approval: Option<&str>,
    input: &Value,
) -> Result<(), String> {
    let mut receipt = json!({
        "schemaVersion": 1,
        "source": "fox_kernel_host",
        "runId": run_id,
        "toolCallId": tool_id,
        "tool": tool,
        "executionState": if completed { "completed" } else { "failed" },
        "approvalDecision": approval.unwrap_or("not_requested_for_this_call"),
    });
    // Only the local write executor has this contract. Never promote arbitrary
    // connector details or model-provided fields to a Host write confirmation.
    if tool == "write_file" {
        if let Some(path) = input["path"].as_str().filter(|path| path.len() <= 4096) {
            receipt["requestedPath"] = json!(path);
        }
        if completed {
            let content = input["content"].as_str();
            let bytes = result["details"]["bytes"].as_u64();
            if let (Some(content), Some(bytes)) = (content, bytes) {
                if bytes == content.len() as u64 {
                    let mut write = json!({
                        "basis": "completed_local_write_result_and_submitted_input",
                        "utf8Bytes": bytes,
                        "contentSha256": hex::encode(Sha256::digest(content.as_bytes())),
                        "independentReadbackPerformed": false,
                        "exactContentIncluded": content.len() <= 2048,
                    });
                    // Preserve exact Unicode/whitespace, or omit completely;
                    // never present a truncated excerpt as the complete file.
                    if content.len() <= 2048 {
                        write["exactSubmittedContent"] = json!(content);
                    }
                    if let Some(operation @ ("created" | "modified")) =
                        result["details"]["operation"].as_str()
                    {
                        write["operation"] = json!(operation);
                    }
                    receipt["write"] = write;
                }
            }
        }
    }
    let content = result
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .ok_or("durable result has no content array")?;
    content.push(json!({"type":"text", "text":format!("FOX_EXECUTION_RECEIPT_V1\n{}", receipt)}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(bytes: usize) -> Value {
        json!({"content":[{"type":"text","text":"original tool output"}],
            "details":{"bytes":bytes,"operation":"created","apiKey":"must-not-be-projected"}})
    }
    fn receipt(result: &Value) -> Value {
        serde_json::from_str(
            result["content"].as_array().unwrap().last().unwrap()["text"]
                .as_str()
                .unwrap()
                .strip_prefix("FOX_EXECUTION_RECEIPT_V1\n")
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn approved_write_reports_exact_unicode_and_bytes_without_claiming_readback() {
        let text = "中文 😀\n";
        let input = json!({"path":"proof.txt","content":text,"approvalDecision":"deny"});
        let mut output = result(text.len());
        append_execution_receipt(
            &mut output,
            "run",
            "call",
            "write_file",
            true,
            Some("allow_once"),
            &input,
        )
        .unwrap();
        let facts = receipt(&output);
        assert_eq!(facts["approvalDecision"], "allow_once");
        assert_eq!(facts["executionState"], "completed");
        assert_eq!(facts["write"]["exactSubmittedContent"], text);
        assert_eq!(facts["write"]["utf8Bytes"], text.len());
        assert_eq!(facts["write"]["independentReadbackPerformed"], false);
        assert!(!facts.to_string().contains("must-not-be-projected"));
        assert_eq!(output["content"][0]["text"], "original tool output");
    }

    #[test]
    fn failure_or_inconsistent_output_never_claims_content_written() {
        for (completed, approval, bytes) in [
            (false, Some("denied"), 3),
            (false, Some("allow_once"), 3),
            (true, None, 99),
        ] {
            let mut output = result(bytes);
            append_execution_receipt(
                &mut output,
                "r",
                "t",
                "write_file",
                completed,
                approval,
                &json!({"content":"abc"}),
            )
            .unwrap();
            assert!(receipt(&output).get("write").is_none());
            assert_eq!(
                receipt(&output)["approvalDecision"],
                approval.unwrap_or("not_requested_for_this_call")
            );
        }
    }

    #[test]
    fn empty_and_large_writes_are_unambiguous_and_bounded() {
        for text in [String::new(), "中".repeat(2000)] {
            let mut output = result(text.len());
            append_execution_receipt(
                &mut output,
                "r",
                "t",
                "write_file",
                true,
                Some("allow_conversation"),
                &json!({"path":"a","content":text}),
            )
            .unwrap();
            let write = &receipt(&output)["write"];
            assert_eq!(write["exactContentIncluded"], text.is_empty());
            if text.is_empty() {
                assert_eq!(write["exactSubmittedContent"], "");
            } else {
                assert!(write.get("exactSubmittedContent").is_none());
            }
            assert!(output["content"][1]["text"].as_str().unwrap().len() < 1000);
        }
    }

    #[test]
    fn connector_payload_cannot_supply_host_write_or_approval_facts() {
        let mut output = result(3);
        output["foxExecutionReceipt"] = json!({"approvalDecision":"allow_once"});
        append_execution_receipt(
            &mut output,
            "r",
            "t",
            "call_mcp_tool",
            true,
            None,
            &json!({"content":"abc"}),
        )
        .unwrap();
        assert!(receipt(&output).get("write").is_none());
        assert_eq!(
            receipt(&output)["approvalDecision"],
            "not_requested_for_this_call"
        );
    }
}
