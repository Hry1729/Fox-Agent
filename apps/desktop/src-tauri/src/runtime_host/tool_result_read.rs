//! `read_tool_result` — the model-facing reader for a tool result Fox already stored.
//!
//! A bounded model view promises that omitted content is still reachable. This is
//! the tool that keeps that promise: it resolves a `fox-result://<runId>/<toolCallId>`
//! reference against `tool_calls` and returns one byte range of the result Host
//! persisted when the call settled.
//!
//! Guarantees this module is responsible for:
//!
//! - **It never executes anything.** Only bytes already stored are returned; the
//!   original tool is not run again, so reads cannot duplicate a write.
//! - **The scope comes from Host, not from the model.** [`prepare`] refuses every
//!   argument that names a conversation or Run, and the executor is handed the
//!   conversation id the Run binding already records.
//! - **It reports the truth about storage.** When Host only kept a preview, the
//!   response says `retrievable: false` instead of implying every byte can be
//!   recovered from here.
//!
//! Nothing here knows about approvals, UI, or transport: it prepares arguments and
//! answers one question against the database. The Kernel context-resource path
//! (`kernel_gateway.rs::execute_context_resource`) and the legacy protocol path
//! (`mod.rs::execute_tool_result_read_request`) are the two integrated callers.

use crate::database::Database;
use serde_json::{json, Value};

/// Bytes returned when the model does not ask for a specific size. Large enough
/// to be useful, small enough that a bounded context is not swamped by one read.
pub(super) const DEFAULT_LIMIT_BYTES: usize = 16_384;

/// Ceiling on one range. Matches `MAX_TOOL_RESULT_RANGE_BYTES` in the database
/// layer, which enforces it again so the two cannot drift unnoticed.
pub(super) const MAX_LIMIT_BYTES: usize = 64 * 1024;

/// Arguments the model may carry. Deliberately free of any field that could name
/// a scope: authorization is bound by Host, never requested by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadToolResultRequest {
    pub reference: String,
    pub offset: usize,
    pub limit: usize,
}

/// Fields a caller may not supply, because honouring them would let the model
/// choose its own authorization scope.
const FORBIDDEN_ARGUMENTS: &[&str] = &[
    "conversationId",
    "conversation",
    "authorizedConversationId",
    "runId",
    "toolCallId",
];

pub(super) fn prepare(input: &Value) -> Result<ReadToolResultRequest, String> {
    for field in FORBIDDEN_ARGUMENTS {
        if input.get(*field).is_some() {
            return Err(format!(
                "'{field}' is not an argument of read_tool_result; Fox binds the read scope to this Run"
            ));
        }
    }
    let reference = input
        .get("reference")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "reference is required and must be a fox-result://<runId>/<toolCallId> value".to_owned()
        })?;
    if crate::kernel_compaction::parse_tool_result_ref(reference).is_none() {
        return Err(format!(
            "'{reference}' is not a Fox tool result reference"
        ));
    }
    let offset = match input.get("offset") {
        None | Some(Value::Null) => 0usize,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "offset must be a non-negative integer number of bytes".to_owned())?
            as usize,
    };
    let requested = match input.get("limit") {
        None | Some(Value::Null) => 0usize,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "limit must be a non-negative integer number of bytes".to_owned())?
            as usize,
    };
    // `limit: 0` means "no preference" rather than "nothing": the database layer
    // rejects limits too small to hold a whole code point, so 0 is resolved to
    // the default instead of becoming an unservable request.
    let limit = if requested == 0 {
        DEFAULT_LIMIT_BYTES
    } else {
        requested.min(MAX_LIMIT_BYTES)
    };
    Ok(ReadToolResultRequest {
        reference: reference.to_owned(),
        offset,
        limit,
    })
}

/// Read one range of a stored result. `authorized_conversation_id` must have been
/// derived from the Run's own binding by the caller.
///
/// The returned `content` block holds only stored bytes — no banner, no escaping —
/// so ranges can be concatenated back into exactly what was persisted. Every other
/// fact (cursor, completeness, retrievability) travels in `details`, matching how
/// `read_attachment` reports paging.
pub(super) fn execute(
    database: &Database,
    authorized_conversation_id: &str,
    request: &ReadToolResultRequest,
) -> Result<Value, String> {
    let range = database.tool_result_range(
        &request.reference,
        authorized_conversation_id,
        request.offset,
        request.limit,
    )?;
    let complete = range.next_offset.is_none();
    let mut details = json!({
        "reference": request.reference,
        "runId": range.run_id,
        "toolCallId": range.tool_call_id,
        "toolName": range.tool_name,
        "status": range.status,
        "offset": range.offset,
        "returnedBytes": range.returned_bytes,
        "nextOffset": range.next_offset,
        "complete": complete,
        "originalBytes": range.original_bytes,
        "truncated": range.truncated,
        "retrievable": range.retrievable,
        "source": if range.retrievable {
            "settled tool result stored by Fox Host"
        } else {
            "bounded Fox preview; Host did not keep every byte, so omitted content cannot be recovered from this reference"
        },
    });
    if let Some(next) = range.next_offset {
        details["reread"] = json!({
            "tool": "read_tool_result",
            "arguments": { "reference": request.reference, "offset": next, "limit": request.limit },
            "instruction": format!(
                "This range ended at byte {}. Call read_tool_result with offset={} to continue; repeat until complete=true.",
                next, next
            ),
        });
    }
    Ok(json!({
        "content": [{ "type": "text", "text": range.content }],
        "details": details,
    }))
}
