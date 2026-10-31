//! Versioned model-view policy. Original execution history is never edited.
#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Tools whose large output is re-readable reference material (help indexes,
/// document reads/stats, data samples) rather than a one-shot fact. Their
/// output may be replaced in the MODEL VIEW by a bounded excerpt the model can
/// page back via the same tool. Execution/fact tools are never bounded.
///
/// `call_mcp_tool` is deliberately absent: a generic MCP call has unknown
/// side-effect and re-read semantics ("call it again" is not guaranteed to
/// return the same value, and may repeat a write), so its result passes
/// through verbatim. `list_mcp_tools` only enumerates the catalog.
///
/// Every name here must be a tool that actually exists in `TOOL_CONTRACTS`;
/// an entry that can never match would advertise a capability nobody has.
/// (`attachment_pages`, `graph_read` and `read_only_graph_read` were removed for
/// exactly that reason — no module registers them.)
const BOUNDABLE_TOOLS: &[&str] = &[
    "office_help",
    "office_read",
    "office_validate",
    "read",
    "read_attachment",
    "query_knowledge_graph",
    "list_mcp_tools",
];

/// Default size bounds of the model view of one tool result (UTF-8 bytes).
/// The persisted result is never truncated; this is view-only.
pub(crate) const TOOL_VIEW_HEAD_BYTES: usize = 6_000;
const TOOL_VIEW_TAIL_BYTES: usize = 2_000;
pub(crate) const TOOL_VIEW_MAX_BYTES: usize = 9_000;
/// The tool the model calls to recover bytes omitted from a bounded view. Its
/// input schema, Run policy entry and Host dispatch are declared in
/// `TOOL_CONTRACTS`, `runtime_host/tool_result_read.rs` and `host-tools.mjs`.
pub(crate) const RESULT_REF_TOOL: &str = "read_tool_result";
const RECEIPT_MARKER: &str = "FOX_EXECUTION_RECEIPT_V1";

/// Trusted storage fact for one settled tool result, produced by the Host on
/// the durable write path (blob + `tool_calls` row) — never inferred from the
/// result's size and never taken from metadata the tool result declares about
/// itself. Mirrors `normalizeToolResultStorage` in
/// `services/agent-runtime/src/tool-view.mjs`.
///
/// `stored: false` or an absent fact means "not verified": the projection must
/// not omit bytes it cannot hand back, and must not promise that
/// `read_tool_result` can return them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolResultStorage {
    pub stored: bool,
    #[serde(default)]
    pub stored_bytes: u64,
    #[serde(default)]
    pub retrievable_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

impl ToolResultStorage {
    /// No trusted fact: nothing may be omitted and nothing promised.
    pub(crate) fn unknown() -> Self {
        Self::default()
    }

    /// The Host confirmed it stored this complete result and can hand back
    /// every byte of it.
    pub(crate) fn whole(total_bytes: usize) -> Self {
        Self {
            stored: true,
            stored_bytes: total_bytes as u64,
            retrievable_bytes: total_bytes as u64,
            blob_sha256: None,
            failure_reason: None,
        }
    }

    /// Whether every byte of a result of `original_bytes` can be read back.
    /// A partial store is not enough: the projection would otherwise omit
    /// bytes it cannot hand back.
    pub(crate) fn covers(&self, original_bytes: usize) -> bool {
        self.stored
            && self.stored_bytes >= original_bytes as u64
            && self.retrievable_bytes >= original_bytes as u64
    }
}

/// Whether bytes omitted from the model view can still be recovered, decided
/// only by the Host's storage fact.
fn result_is_retrievable(storage: &ToolResultStorage, total_bytes: usize) -> bool {
    storage.covers(total_bytes)
}

/// How to reach bytes this projection omitted, for the model to act on. A
/// promise is made only when the Host confirmed complete storage.
fn retrieval_instruction(
    reference: Option<&str>,
    from_offset: Option<usize>,
    retrievable: bool,
) -> String {
    match (reference, retrievable) {
        (Some(reference), true) => {
            let arguments = match from_offset {
                Some(offset) if offset > 0 => {
                    format!("{{\"reference\":\"{reference}\",\"offset\":{offset}}}")
                }
                _ => format!("{{\"reference\":\"{reference}\"}}"),
            };
            format!(
                "调用 {RESULT_REF_TOOL} {arguments} 可只读取回 Host 已保存的本次结果（不重新执行任何工具，也不会重放写操作）；返回 details.complete=true 前按 details.nextOffset 继续接力即可取得全部原文。"
            )
        }
        (Some(reference), false) => format!(
            "本次结果的完整存储未获 Host 确认（引用 {reference} 仅标识该次调用）。被省略的部分无法保证可取回，请用同一只读工具以更精确的 selector/页码重新查询；不要凭省略内容编造字段。"
        ),
        (None, _) => {
            "本次未附带结果引用，也未获得 Host 已完整存储的确认，请改用同一只读工具以更精确的 selector/页码重新查询。".to_owned()
        }
    }
}
/// Longest single string leaf kept verbatim by a structural projection. The
/// projection lowers this progressively until the JSON fits the budget.
const STRUCT_FIELD_BYTES: usize = 160;
const STRUCT_FIELD_BYTES_FLOOR: usize = 16;
const STRUCT_VIEW_KEY: &str = "foxModelView";

/// Smallest projection that can still name the payload and carry `next`; used
/// to decide whether navigation can be carried at all.
const MIN_STRUCTURED_ENVELOPE_BYTES: usize = 384;

fn is_boundable(tool: &str) -> bool {
    BOUNDABLE_TOOLS.contains(&tool)
}

/// The name to judge view-bounding by, when `tool` is a real boundable tool.
///
/// A caller that unwraps a dispatch wrapper (the Host reaches every Office
/// operation through `call_mcp_tool`) must ask here rather than keep its own
/// list, so the whitelist stays the single source of truth and cannot be
/// widened by accident: removing a name from `BOUNDABLE_TOOLS` also stops the
/// unwrapping for it.
pub(crate) fn boundable_tool_name(tool: &str) -> Option<&str> {
    is_boundable(tool).then_some(tool)
}

/// Stable reference to the durable *record* of one settled tool call. Host keys
/// that record by the pair (run id, tool call id) — `tool_calls.run_id` together
/// with `tool_calls.runtime_tool_call_id`, which the table declares UNIQUE — and
/// `Database::tool_result_range` resolves the reference back to stored bytes
/// under the owning conversation's authorization (see `parse_tool_result_ref`).
///
/// The stored copy is **not** unbounded: `MAX_STORED_TOOL_RESULT_BYTES` (128 KiB)
/// caps `tool_calls.result_json`, and above it Host stores a bounded preview
/// with `truncated: true`. The reader reports that explicitly instead of
/// pretending the full text is available, so this reference is never presented
/// as a guarantee of complete historical content.
/// Mirrors `toolResultRef` in `services/agent-runtime/src/tool-view.mjs`.
pub(crate) fn tool_result_ref(run_id: &str, tool_call_id: &str) -> Option<String> {
    if run_id.is_empty() || tool_call_id.is_empty() {
        return None;
    }
    Some(format!("fox-result://{run_id}/{tool_call_id}"))
}

/// Parse `fox-result://<runId>/<toolCallId>` back into its identity pair. The
/// Host-side counterpart of `tool_result_ref`; returns `None` for anything that
/// is not exactly one run id and one call id.
pub(crate) fn parse_tool_result_ref(reference: &str) -> Option<(String, String)> {
    let rest = reference.strip_prefix("fox-result://")?;
    let (run_id, tool_call_id) = rest.split_once('/')?;
    if run_id.is_empty()
        || tool_call_id.is_empty()
        || tool_call_id.contains('/')
        || run_id.len() > 512
        || tool_call_id.len() > 512
    {
        return None;
    }
    Some((run_id.to_owned(), tool_call_id.to_owned()))
}

/// Replace over-long string leaves while keeping keys, order and item count.
/// `preserve` keeps a subtree verbatim; navigation is carried separately by
/// `split_next`, so nothing here rewrites or shortens it.
fn replace_long_strings(value: &Value, field_bytes: usize, preserve: bool) -> Value {
    match value {
        Value::String(text) => {
            if preserve || text.len() <= field_bytes {
                Value::String(text.clone())
            } else {
                Value::String(format!(
                    "{}…",
                    bounded_prefix(text, field_bytes.saturating_sub(3))
                ))
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| replace_long_strings(item, field_bytes, preserve))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, child)| {
                    (key.clone(), replace_long_strings(child, field_bytes, preserve))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn view_note(
    total_bytes: usize,
    reference: Option<&str>,
    retrievable: bool,
    storage_verified: bool,
) -> Value {
    let mut note = json!({
        "bounded": true,
        "originalBytes": total_bytes,
        "budgetBytes": TOOL_VIEW_MAX_BYTES,
        // Whether every omitted byte can still be reached, and whether that
        // claim rests on a trusted Host fact rather than on a size guess.
        "retrievable": retrievable,
        "storageVerified": storage_verified,
    });
    if let Some(reference) = reference {
        note["resultRef"] = json!(reference);
        note["resultRefNote"] = json!(retrieval_instruction(Some(reference), None, retrievable));
    }
    note["reason"] = json!(if retrievable {
        "模型视图有界化：原始结果对象未被修改。被省略的内容已由 Host 完整存储，可按上述引用取回，不要凭省略内容编造字段。"
    } else {
        "模型视图有界化：原始结果对象未被修改。Host 未确认完整存储，本视图不省略任何无法取回的记录。"
    });
    note
}

fn with_view_note(payload: &Value, note: &Value, is_array: bool) -> Value {
    if is_array {
        return json!({STRUCT_VIEW_KEY: note, "foxBoundedList": payload});
    }
    match payload.as_object() {
        Some(map) => {
            let mut out = map.clone();
            out.insert(STRUCT_VIEW_KEY.to_string(), note.clone());
            Value::Object(out)
        }
        None => json!({STRUCT_VIEW_KEY: note, "foxValue": payload}),
    }
}

/// Split a top-level `next` navigation entry off the payload, so it can be
/// re-attached verbatim after the body has been projected. Navigation is what
/// makes the omitted records reachable; it is therefore never shortened and
/// never dropped independently of the records.
fn split_next(payload: &Value) -> (Value, Option<Value>) {
    match payload {
        Value::Object(map) => {
            let mut body = map.clone();
            let next = body.remove("next");
            (Value::Object(body), next)
        }
        other => (other.clone(), None),
    }
}

fn largest_array_key(value: &Value) -> Option<String> {
    value
        .as_object()?
        .iter()
        .filter(|(_, child)| child.is_array())
        .max_by_key(|(_, child)| child.as_array().map_or(0, Vec::len))
        .map(|(key, _)| key.clone())
}

/// Compose the bounded model view: projected body + note + `next` verbatim.
fn compose_view(body: &Value, note: &Value, is_array: bool, next: Option<&Value>) -> Value {
    let mut out = with_view_note(body, note, is_array);
    if let (Some(next), Some(map)) = (next, out.as_object_mut()) {
        map.insert("next".to_owned(), next.clone());
    }
    out
}

/// Continuation that resumes *exactly* at the first record the projection
/// omitted, for the one paged boundable tool (`office_help`).
///
/// `office.rs::shape_help` slices a sorted catalog with
/// `start = (page - 1) * pageSize`, `1 <= pageSize <= 60`. An absolute index
/// `resume_index` is therefore reachable by choosing a `pageSize` that divides
/// it (`1` always divides) with `page = resume_index / pageSize + 1`; the
/// returned arguments use only fields the tool's real input schema declares
/// (`format`, `element`, `page`, `pageSize`). Returns `None` when the payload is
/// not that contract, which makes the caller keep every record instead of
/// dropping records it cannot point back to.
/// The one paged contract this module can prove, exactly as
/// `apps/desktop/src-tauri/src/office.rs::shape_help` implements it: only
/// `office_help` pages a catalog, it addresses records as
/// `start = (page - 1) * pageSize`, and its input schema declares `format`
/// (required), `element`, `page` and `pageSize` with `1 <= pageSize <= 60`.
///
/// Returns the fields needed to resume, or `None` for any payload whose
/// continuation cannot be derived — including every other tool, whose cursor
/// semantics this module has no evidence for and must not invent.
fn paged_contract(payload: &Value) -> Option<(String, String, u64, u64)> {
    let format = payload
        .get("format")?
        .as_str()
        .filter(|value| !value.is_empty())?
        .to_owned();
    let element = payload
        .get("element")?
        .as_str()
        .filter(|value| !value.is_empty())?
        .to_owned();
    let page = payload.get("page")?.as_u64()?;
    let page_size = payload.get("pageSize")?.as_u64()?;
    if page < 1 || !(1..=60).contains(&page_size) {
        return None;
    }
    Some((format, element, page, page_size))
}

/// Continuation that resumes exactly at the first record the projection omitted.
///
/// Absolute index `resume_index` is reachable by any `pageSize` dividing it
/// (`1` always divides), with `page = resume_index / pageSize + 1`. Only fields
/// the tool's real input schema declares are emitted. `None` for anything whose
/// page cannot be derived, which makes the caller keep every record instead of
/// dropping records it cannot point back to.
/// Mirrors `pagedResume` in `tool-view.mjs`.
fn paged_resume(payload: &Value, collection_key: &str, kept: usize) -> Option<Value> {
    if collection_key != "properties" {
        return None;
    }
    let (format, element, page, page_size) = paged_contract(payload)?;
    let resume_index = (page - 1)
        .checked_mul(page_size)?
        .checked_add(u64::try_from(kept).ok()?)?;
    // `resume_index == 0` is legal: it re-reads from the first record, and
    // paging forward from page 1 still reaches every later record.
    let mut page_size_out = 1u64;
    for divisor in 1..=60u64 {
        if resume_index % divisor == 0 {
            page_size_out = divisor;
        }
    }
    Some(json!({
        "tool": "office_help",
        "arguments": {
            "format": format,
            "element": element,
            "page": resume_index / page_size_out + 1,
            "pageSize": page_size_out,
        },
        "instruction": format!(
            "This page was shortened for the model view. Records from index {resume_index} on are not in this response; echoing this continuation resumes exactly at record {} (pageSize={page_size_out}) and paging forward reaches every remaining record.",
            resume_index + 1
        ),
    }))
}

/// Project a parsed JSON payload so it stays valid JSON within `max_bytes`.
/// Keys, item order and the `next` navigation survive; records are dropped only
/// together with a continuation that provably reaches them under the tool's real
/// schema. Returns `None` when no honest projection fits, so the caller keeps the
/// original content rather than publishing a view that lies about navigation.
/// Mirrors `projectStructuredText` in `tool-view.mjs`.
pub(crate) fn project_structured_text(
    tool: &str,
    parsed: &Value,
    max_bytes: usize,
    reference: Option<&str>,
    storage: &ToolResultStorage,
) -> Option<String> {
    let _ = tool;
    if !parsed.is_object() && !parsed.is_array() {
        return None;
    }
    let is_array = parsed.is_array();
    let total_bytes = crate::canonical_json::to_string(parsed).ok()?.len();
    // Decided once for the whole payload: omitting anything is only honest while
    // the omitted bytes stay reachable, and reachability comes from the Host's
    // storage fact — never from the payload's size.
    let retrievable = result_is_retrievable(storage, total_bytes);
    // String leaves and the last-resort envelope are lossy too. Only a
    // verified complete historical result with a scoped reference may shrink.
    if !retrievable || reference.is_none() {
        return None;
    }
    let note = view_note(total_bytes, reference, retrievable, storage.stored);

    let (body, next) = split_next(parsed);
    let next_bytes = match &next {
        Some(value) => crate::canonical_json::to_string(value).ok()?.len(),
        None => 0,
    };
    // Navigation is reserved *inside* the budget. If it cannot be carried, do
    // not bound this result at all instead of silently dropping it.
    if next_bytes.saturating_add(MIN_STRUCTURED_ENVELOPE_BYTES) > max_bytes {
        return None;
    }

    // 1. Shrink over-long string leaves only: every key, every record and the
    //    navigation entry survive untouched.
    let mut field_bytes = STRUCT_FIELD_BYTES;
    while field_bytes >= STRUCT_FIELD_BYTES_FLOOR {
        let candidate = compose_view(
            &replace_long_strings(&body, field_bytes, false),
            &note,
            is_array,
            next.as_ref(),
        );
        if let Ok(text) = crate::canonical_json::to_string(&candidate) {
            if text.len() <= max_bytes {
                return Some(text);
            }
        }
        field_bytes /= 2;
    }

    // 2. The body still does not fit. Drop trailing records from the largest
    //    collection — but only when the omitted records stay reachable. With a
    //    `next` present that means emitting a continuation which resumes at the
    //    first omitted record; without one there is no navigation to break, and
    //    the omission is reported explicitly so the model narrows its re-read.
    if !is_array {
        let shrunk = replace_long_strings(&body, STRUCT_FIELD_BYTES_FLOOR, false);
        if let Some(key) = largest_array_key(&shrunk) {
            let full = shrunk
                .get(&key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            // Reachability is independent of how many records are kept, so it is
            // decided once: the paged contract can be resumed for any keep
            // count, anything else must keep every record it cannot point back
            // to.
            let resumable = key == "properties" && paged_contract(parsed).is_some();
            if next.is_some() && !resumable {
                return None;
            }
            // Records may leave the view only when they stay reachable: either
            // the paged contract can be replayed, or Host kept the whole result
            // *and* the view carries the reference `read_tool_result` needs to
            // walk it. Neither is true here, so the result is left unbounded
            // rather than losing records everywhere.
            if !resumable && !(retrievable && reference.is_some()) {
                return None;
            }
            let mut keep = full.len();
            while keep > 0 {
                keep /= 2;
                let mut dropped = note.clone();
                dropped["omittedItems"] = json!(full.len() - keep);
                dropped["collectionKey"] = json!(key);
                if dropped["omittedItems"] != json!(0) {
                    let mut advice = if resumable {
                        "用同一只读工具按 next 的页码继续查询即可取回".to_owned()
                    } else {
                        format!(
                            "{} ",
                            retrieval_instruction(reference, None, retrievable)
                        )
                    };
                    if resumable && retrievable {
                        if let Some(extra) = reference {
                            advice.push_str(&format!(
                                "；也可调用 {RESULT_REF_TOOL} {{\"reference\":\"{extra}\"}} 按范围读回本次保存的原文"
                            ));
                        }
                    }
                    advice.push_str("；不要凭被省略的内容编造字段，也不要为读取历史结果而重放任何写操作。");
                    dropped["reRead"] = json!(format!(
                        "本响应未包含 collectionKey={key} 的前 {keep} 项之后的记录；{advice}"
                    ));
                }
                let composed_next = match next.as_ref() {
                    Some(original) => {
                        paged_resume(parsed, &key, keep).or_else(|| Some(original.clone()))
                    }
                    None => None,
                };
                let mut body_at = shrunk.clone();
                if let Value::Object(map) = &mut body_at {
                    map.insert(key.clone(), Value::Array(full[..keep].to_vec()));
                }
                let candidate = compose_view(&body_at, &dropped, false, composed_next.as_ref());
                if let Ok(text) = crate::canonical_json::to_string(&candidate) {
                    if text.len() <= max_bytes {
                        return Some(text);
                    }
                }
            }
        }
    }

    // 3. Last resort: a minimal valid envelope that still carries the
    //    navigation entry verbatim. Never an invalid JSON fragment, never a
    //    dropped `next`. Reached only when records were allowed to leave the
    //    view in step 2 — i.e. when they are reachable.
    let mut fallback_note = view_note(total_bytes, reference, retrievable, storage.stored);
    fallback_note["omitted"] = json!(true);
    fallback_note["note"] = json!(retrieval_instruction(reference, None, retrievable));
    let fallback = compose_view(&Value::Null, &fallback_note, false, next.as_ref());
    let text = crate::canonical_json::to_string(&fallback).ok()?;
    (text.len() <= max_bytes).then_some(text)
}

/// Serialized `next` navigation of a JSON object, kept verbatim by the text
/// fallback so a paged read is never left without its continuation.
fn navigation_of(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    let next = parsed.get("next").filter(|value| !value.is_null())?;
    crate::canonical_json::to_string(next).ok()
}

/// Bound one tool result text block. Returns `Some(replacement)` for the text
/// the model view must publish, `None` when the caller keeps the original
/// (small, receipt-bearing, non-text-block, or non-boundable tool).
///
/// A structured (JSON) payload that no honest projection can carry is returned
/// **unchanged** rather than cut: a head/tail cut would produce invalid JSON and
/// hide records whose continuation was dropped with it. Returning the original
/// is the truthful model view — nothing omitted, nothing to reach for.
///
/// `storage` is the Host's trusted storage fact for this result; without it no
/// omission and no retrieval promise is made.
pub(crate) fn bound_tool_text(
    tool: &str,
    is_error: bool,
    text: &str,
    reference: Option<&str>,
    storage: &ToolResultStorage,
) -> Option<String> {
    if is_error || !is_boundable(tool) || text.len() <= TOOL_VIEW_MAX_BYTES {
        return None;
    }
    // A receipt block is execution evidence and is never reshaped.
    if text.contains(RECEIPT_MARKER) {
        return None;
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
            return match project_structured_text(tool, &parsed, TOOL_VIEW_MAX_BYTES, reference, storage)
            {
                Some(projected) => Some(projected),
                // No honest structured view: keep the payload intact. `next`
                // alone may exceed the whole budget, and omitting records it
                // cannot point back to would make the view lie about navigation.
                None => Some(text.to_owned()),
            };
        }
    }
    // A head/tail cut keeps the beginning and the end but loses the middle, so
    // it is only honest while the omitted bytes can be walked back: the Host
    // must have confirmed complete storage and the view must carry the
    // reference. Both conditions come from trusted facts, never from size.
    if !(result_is_retrievable(storage, text.len()) && reference.is_some()) {
        return Some(text.to_owned());
    }
    let head = bounded_prefix(text, TOOL_VIEW_HEAD_BYTES);
    let tail = bounded_suffix(text, TOOL_VIEW_TAIL_BYTES);
    let omitted = text
        .len()
        .saturating_sub(head.len())
        .saturating_sub(tail.len());
    let mut notice = format!(
        "[Fox 已为本条工具结果建立有界视图：原文共 {} 字节，模型上下文中保留开头 {} 与结尾 {} 字节（原始结果对象未被修改）。",
        text.len(),
        head.len(),
        tail.len()
    );
    notice.push_str(&retrieval_instruction(reference, None, true));
    notice.push_str(
        "也可以按需用同一只读工具以更精确的 selector/页码重新查询——office_help 用 property=<属性名> 或 page=<页码>，office_read/read 用更精确的 selector 或更小范围；不要凭被省略的内容编造字段，也不要为读取历史结果而重复任何写操作。]",
    );
    let mut out = format!("{head}\n\n{notice}\n…[省略 {omitted} 字节，可用同一只读工具按需取回]…\n\n{tail}");
    // Navigation is the one part that is kept outside the head/tail budget: a
    // text-bound view that drops `next` would silently hide the records it
    // claims are reachable.
    if let Some(next) = navigation_of(text) {
        out.push_str(&format!("\n\n[\"next\" 导航原样保留] {next}"));
    }
    Some(out)
}

/// Compatibility entry for call sites that do not yet carry the Host's trusted
/// storage fact (e.g. `live.rs` until interface-request R4-A1 is wired). It is
/// deliberately conservative: with no verified storage the projection keeps
/// every byte and promises nothing, so behaviour is safe but large results stay
/// unbounded. Switch call sites to `bound_tool_result_content_with_storage`
/// once the fact is available.
pub(crate) fn bound_tool_result_content(
    tool: &str,
    is_error: bool,
    content: &Value,
    reference: Option<&str>,
) -> Option<Value> {
    bound_tool_result_content_with_storage(
        tool,
        is_error,
        content,
        reference,
        &ToolResultStorage::unknown(),
    )
}

/// Bound a tool result's model-visible content while the source result object
/// stays untouched. Receipt/approval/execution blocks and error results are
/// preserved verbatim; only large re-readable reference output is replaced —
/// structurally for JSON (keys, records, and `next` navigation survive) and on
/// UTF-8 code-point boundaries for prose. `reference` identifies the durable
/// record for the model and `storage` is the Host's trusted storage fact.
/// Returns `Some(new_content)` when at least one block actually changed, `None`
/// when every block stays as it was.
pub(crate) fn bound_tool_result_content_with_storage(
    tool: &str,
    is_error: bool,
    content: &Value,
    reference: Option<&str>,
    storage: &ToolResultStorage,
) -> Option<Value> {
    if is_error || !is_boundable(tool) {
        return None;
    }
    let blocks = content.as_array()?;
    let mut changed = false;
    let mut new_blocks = Vec::with_capacity(blocks.len());
    for block in blocks {
        if block["type"].as_str() != Some("text") {
            new_blocks.push(block.clone());
            continue;
        }
        let text = block["text"].as_str().unwrap_or_default();
        match bound_tool_text(tool, false, text, reference, storage) {
            // A structured payload returned unchanged is not a change.
            Some(bounded) if bounded != text => {
                changed = true;
                new_blocks.push(json!({"type":"text","text":bounded}));
            }
            _ => new_blocks.push(block.clone()),
        }
    }
    if changed {
        Some(Value::Array(new_blocks))
    } else {
        None
    }
}

/// The operation a settled call is judged by when bounding the model view.
///
/// `office_read` is named in `BOUNDABLE_TOOLS`, but the Host never dispatches it
/// under that name: the built-in connector is always reached through the
/// `call_mcp_tool` wrapper, so a match on the wrapper name made the whitelist
/// entry dead code and let a 927-row sheet read pass through verbatim.
///
/// Only the built-in Office connector is unwrapped, and only for the read-only
/// operations the whitelist already accepts. A *generic* MCP call keeps
/// `call_mcp_tool`: its side-effect and re-read semantics are unknown, which is
/// what the whitelist's own reasoning requires. An unrecognised or write-form
/// inner operation also keeps the wrapper name, so it is never bounded.
///
/// This is the single definition: `runtime_host::kernel_coordinator::live` uses
/// it for the frame it sends the worker, and the admission-side delivery replay
/// below uses it for the durable row, so the two can never disagree about which
/// operation the projected bytes belong to. Mirrors `effectiveBoundableTool` in
/// `services/agent-runtime/src/tool-view.mjs`.
pub(crate) fn effective_boundable_tool<'a>(tool: &'a str, canonical_input: &'a Value) -> &'a str {
    const UNWRAPPABLE: &[&str] = &["office_read", "office_help", "office_validate"];
    if tool != "call_mcp_tool" {
        return tool;
    }
    if canonical_input["serverId"].as_str() != Some(crate::office::SERVER_ID) {
        return tool;
    }
    match canonical_input["tool"].as_str() {
        Some(inner) if UNWRAPPABLE.contains(&inner) => boundable_tool_name(inner).unwrap_or(tool),
        _ => tool,
    }
}

/// Conservative whole-result eligibility under the current model projection.
///
/// This is a replayed eligibility calculation, not a durable historical record
/// of a Provider request. It supplements the Host's SOURCE fact
/// (`HostObservation::authorizes_whole_file_replacement`). A whole-file read
/// whose model projection was head/tail-bounded cannot qualify here.
///
/// It replays the production Kernel projection against the durable result,
/// never against an already-projected view. Legacy's actual model path requires
/// separate evidence. Two consequences are load-bearing:
///
/// * With the same canonical result, verified storage and projection rules,
///   repeated calculation gives the same answer. Missing or inconsistent
///   evidence can only lower eligibility.
/// * `true` requires a successful, readable result and a verified durable
///   storage fact. A failed result or an unknown storage fact cannot establish
///   that a full-file read reached the model.
pub(crate) fn model_view_keeps_every_byte(
    tool: &str,
    is_error: bool,
    content: &Value,
    reference: Option<&str>,
    storage: &ToolResultStorage,
) -> bool {
    let Some(blocks) = content.as_array() else { return false; };
    let mut text_bytes = 0usize;
    let mut text_blocks = 0usize;
    for block in blocks {
        if block["type"] == "text" {
            let Some(text) = block["text"].as_str() else { return false; };
            text_bytes = text_bytes.saturating_add(text.len());
            text_blocks += 1;
        }
    }
    text_bytes = text_bytes.saturating_add(text_blocks.saturating_sub(1));
    if is_error || !is_boundable(tool) || text_blocks == 0 || reference.is_none()
        || !storage.stored || storage.failure_reason.is_some()
        || storage.retrievable_bytes != storage.stored_bytes
        || !storage.covers(text_bytes) {
        return false;
    }
    bound_tool_result_content_with_storage(tool, false, content, reference, storage).is_none()
}

/// Read a settled result's actual content blocks. A missing, null or malformed
/// content cannot prove a full-file read; serializing the envelope as fallback
/// text would manufacture a model delivery that never occurred.
pub(crate) fn durable_result_content(raw: &Value) -> Option<&Value> {
    let content = raw.get("content")?;
    let blocks = content.as_array()?;
    if blocks.is_empty() || !blocks.iter().any(|block| block["type"] == "text" && block["text"].is_string())
        || blocks.iter().any(|block| block["type"] == "text" && !block["text"].is_string()) {
        return None;
    }
    Some(content)
}

/// Largest UTF-8-safe prefix no longer than `max` bytes.
fn bounded_prefix(text: &str, max: usize) -> &str {
    if text.len() <= max { return text; }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) { end -= 1; }
    &text[..end]
}
fn bounded_suffix(text: &str, max: usize) -> &str {
    if text.len() <= max { return text; }
    let mut start = text.len() - max;
    while start < text.len() && !text.is_char_boundary(start) { start += 1; }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference plus the Host's trusted storage fact is what makes an
    /// omitted byte reachable, so every fixture that expects a bounded view
    /// carries both — as every production caller will once R4-A1 is wired
    /// (`live.rs` and `pi-kernel-loop.mjs` mint the reference per settled call;
    /// the storage fact comes from the durable write path).
    const TEST_REF: &str = "fox-result://run-1/call-1";

    fn stored_whole(total_bytes: usize) -> ToolResultStorage {
        ToolResultStorage::whole(total_bytes)
    }

    #[test]
    fn f1_replay_requires_complete_storage_and_valid_content() {
        let small = json!([{"type":"text","text":"small"}]);
        let empty = json!([{"type":"text","text":""}]);
        assert!(model_view_keeps_every_byte("read", false, &small,
            Some(TEST_REF), &stored_whole(5)));
        assert!(model_view_keeps_every_byte("read", false, &empty,
            Some(TEST_REF), &stored_whole(0)), "empty files are complete files");
        assert!(!model_view_keeps_every_byte("read", false, &small,
            Some(TEST_REF), &ToolResultStorage::unknown()));
        let mut short_store = stored_whole(5);
        short_store.stored_bytes = 4;
        assert!(!model_view_keeps_every_byte("read", false, &small,
            Some(TEST_REF), &short_store));
        let mut short_retrieval = stored_whole(5);
        short_retrieval.retrievable_bytes = 4;
        assert!(!model_view_keeps_every_byte("read", false, &small,
            Some(TEST_REF), &short_retrieval));
        assert!(!model_view_keeps_every_byte("read", true, &small,
            Some(TEST_REF), &stored_whole(5)));
        assert!(!model_view_keeps_every_byte("unknown_reader", false, &small,
            Some(TEST_REF), &stored_whole(5)));
        assert!(!model_view_keeps_every_byte("read", false,
            &json!([{"type":"text","text":null}]), Some(TEST_REF), &stored_whole(4)));
        assert!(!model_view_keeps_every_byte("read", false, &Value::Null,
            Some(TEST_REF), &stored_whole(4)));
        assert!(durable_result_content(&json!({"content":null})).is_none());
        assert!(durable_result_content(&Value::Null).is_none());
        assert!(durable_result_content(&json!({"content":[{"type":"text","text":null}]})).is_none());
    }

    /// Test helper: the same call with the Host's complete-store fact, which is
    /// what the production transport supplies once R4-A1 is wired.
    fn bounded_with_store(
        tool: &str,
        is_error: bool,
        text: &str,
        reference: Option<&str>,
    ) -> Option<String> {
        bound_tool_text(tool, is_error, text, reference, &stored_whole(text.len()))
    }

    #[test]
    fn bounded_tool_view_keeps_errors_receipts_and_non_reference_tools() {
        let big = "x".repeat(50_000);
        let big_content = json!([{"type":"text","text":big}]);
        // Re-readable reference tool with a confirmed complete store is bounded.
        let bounded = bound_tool_result_content_with_storage(
            "office_help",
            false,
            &big_content,
            Some(TEST_REF),
            &stored_whole(big.len()),
        )
        .unwrap();
        let text = bounded[0]["text"].as_str().unwrap();
        assert!(text.len() < big.len());
        assert!(text.contains("有界视图"));
        assert!(text.contains("office_help"));
        // Still a valid toolResult text block.
        assert_eq!(bounded[0]["type"], "text");
        // Without the storage fact nothing is bounded or promised: the compat
        // entry reports "unchanged" (None), so the caller keeps every byte.
        assert!(
            bound_tool_result_content("office_help", false, &big_content, Some(TEST_REF)).is_none(),
            "no verified storage means the projection declines and the payload is kept whole"
        );
        // Errors pass through verbatim so the model can correct them.
        assert!(bound_tool_result_content("office_help", true, &big_content, None).is_none());
        // Execution/fact tools are never bounded.
        assert!(bound_tool_result_content("attachment_compute", false, &big_content, None).is_none());
        assert!(bound_tool_result_content("write_file", false, &big_content, None).is_none());
        // Receipt-bearing reference text is protected.
        let receipt = json!([{"type":"text","text":format!("{big}\nFOX_EXECUTION_RECEIPT_V1 {{}}")}]);
        assert!(bound_tool_result_content("read", false, &receipt, None).is_none());
        // Small content is untouched.
        let small = json!([{"type":"text","text":"short"}]);
        assert!(bound_tool_result_content("office_help", false, &small, None).is_none());
    }

    #[test]
    fn storage_fact_decides_retrievability_instead_of_the_payload_size() {
        // Every one of these payload sizes used to be judged by the retired
        // 128 KiB inference; now only the Host fact decides.
        for total in [50_000usize, 140_000, 200_000, 1_600_000] {
            let big = "x".repeat(total);
            let content = json!([{"type":"text","text":big}]);
            // Confirmed complete store: bounded view naming the reference.
            let bounded = bound_tool_result_content_with_storage(
                "read",
                false,
                &content,
                Some(TEST_REF),
                &stored_whole(total),
            )
            .expect("a stored result must be projected");
            let view = bounded[0]["text"].as_str().unwrap();
            assert!(
                view.len() <= TOOL_VIEW_MAX_BYTES,
                "size {total}: view must stay in budget"
            );
            assert!(view.contains(TEST_REF));
            assert!(view.contains(RESULT_REF_TOOL));
            // Unverified, partial, or failed store: nothing dropped, nothing promised.
            for storage in [
                ToolResultStorage::unknown(),
                ToolResultStorage {
                    stored: true,
                    // A partial store (Host kept only a preview): never enough.
                    stored_bytes: (total / 2) as u64,
                    retrievable_bytes: (total / 2) as u64,
                    ..Default::default()
                },
                ToolResultStorage {
                    stored: false,
                    failure_reason: Some("blob_write_failed".into()),
                    ..Default::default()
                },
            ] {
                // `None` is "unchanged": the caller keeps the original content.
                let kept = match bound_tool_result_content_with_storage(
                    "read",
                    false,
                    &content,
                    Some(TEST_REF),
                    &storage,
                ) {
                    Some(view) => view[0]["text"].as_str().unwrap().to_owned(),
                    None => big.clone(),
                };
                assert_eq!(
                    kept, big,
                    "size {total}: no byte may be omitted without a trusted complete-store fact"
                );
            }
        }
    }

    /// Reference implementation of the probe catalog: 40 properties whose 160-char
    /// CJK hints push the payload just under the 24 KiB Office page budget.
    fn office_catalog() -> Value {
        let properties = (0..40)
            .map(|index| {
                json!({
                    "name": format!("property{index}"),
                    "type": "string",
                    "ops": ["set"],
                    "hint": "中".repeat(160),
                })
            })
            .collect::<Vec<_>>();
        json!({
            "format": "xlsx", "element": "chart", "page": 1, "pageSize": 40,
            "totalProperties": 80, "properties": properties, "complete": false,
            "next": {"tool": "office_help",
                "arguments": {"format": "xlsx", "element": "chart", "page": 2, "pageSize": 40}},
        })
    }

    #[test]
    fn bounded_multibyte_text_never_introduces_a_replacement_character() {
        // A naive byte cut would split the last byte of a CJK character and
        // decode a replacement character in its place.
        let input = format!("a{}", "中".repeat(5_000));
        let bounded = bounded_with_store("office_help", false, &input, Some(TEST_REF)).unwrap();
        assert!(!bounded.contains('\u{FFFD}'), "introduced U+FFFD replacement characters");
        assert!(bounded.len() <= TOOL_VIEW_MAX_BYTES, "bounded view stayed in budget");
        // Emoji are 4-byte code points: the same rule must hold for surrogates.
        let emoji = "😀".repeat(4_000);
        let bounded = bounded_with_store("read", false, &emoji, Some(TEST_REF)).unwrap();
        assert!(!bounded.contains('\u{FFFD}'));
        assert!(bounded.len() <= TOOL_VIEW_MAX_BYTES);
    }

    #[test]
    fn bounded_prefix_and_suffix_land_on_code_point_boundaries() {
        let text = "a中b😀c"; // 'a'=1, '中'=3, 'b'=1, '😀'=4, 'c'=1
        assert_eq!(bounded_prefix(text, 2), "a");
        assert_eq!(bounded_prefix(text, 4), "a中");
        assert_eq!(bounded_prefix(text, 7), "a中b");
        assert_eq!(bounded_prefix(text, 9), "a中b😀");
        assert_eq!(bounded_suffix(text, 1), "c");
        assert_eq!(bounded_suffix(text, 5), "😀c");
        assert_eq!(bounded_suffix(text, 6), "b😀c");
    }

    #[test]
    fn structured_office_catalog_stays_valid_json_with_every_property_and_the_next_page() {
        let catalog = office_catalog();
        let text = catalog.canonical_string();
        assert!(text.len() < 24 * 1024, "fixture must sit inside the Office page budget");
        let bounded = bounded_with_store("office_help", false, &text, Some(TEST_REF)).unwrap();
        assert!(
            bounded.len() <= TOOL_VIEW_MAX_BYTES,
            "structured projection must stay in budget, got {}",
            bounded.len()
        );
        // Before this fix a character cut made this invalid JSON and lost the
        // middle of the catalog, including property20 and the next page entry.
        let parsed: Value = serde_json::from_str(&bounded).expect("bounded catalog stays valid JSON");
        let names = parsed["properties"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, (0..40).map(|index| format!("property{index}")).collect::<Vec<_>>());
        assert_eq!(parsed["next"]["arguments"], catalog["next"]["arguments"]);
        assert_eq!(parsed["next"]["tool"], "office_help");
        assert_eq!(parsed[STRUCT_VIEW_KEY]["bounded"], true);
    }

    #[test]
    fn structured_projection_keeps_navigation_verbatim_and_falls_back_to_valid_json() {
        let catalog = office_catalog();
        let bounded = bounded_with_store("office_help", false, &catalog.canonical_string(), Some(TEST_REF)).unwrap();
        let parsed: Value = serde_json::from_str(&bounded).unwrap();
        assert_eq!(parsed["next"]["tool"], "office_help");
        // A payload with no shrinkable structure still yields parseable JSON.
        let rows = json!((0..200)
            .map(|index| json!({"name": format!("row-{index}"), "blob": "z".repeat(4_000)}))
            .collect::<Vec<_>>());
        let bounded = bounded_with_store("list_mcp_tools", false, &rows.canonical_string(), Some(TEST_REF)).unwrap();
        assert!(bounded.len() <= TOOL_VIEW_MAX_BYTES);
        let parsed: Value = serde_json::from_str(&bounded).expect("fallback stays valid JSON");
        assert_eq!(parsed[STRUCT_VIEW_KEY]["bounded"], true);
    }

    #[test]
    fn bounded_view_reports_a_stable_result_reference_and_small_content_is_untouched() {
        let reference = tool_result_ref("run-1", "call-9").unwrap();
        assert_eq!(reference, "fox-result://run-1/call-9");
        let prose = "y".repeat(40_000);
        let bounded = bounded_with_store("read", false, &prose, Some(&reference)).unwrap();
        assert!(bounded.contains("fox-result://run-1/call-9"));
        let catalog = office_catalog().canonical_string();
        let bounded = bounded_with_store("office_help", false, &catalog, Some(&reference)).unwrap();
        let parsed: Value = serde_json::from_str(&bounded).unwrap();
        assert_eq!(parsed[STRUCT_VIEW_KEY]["resultRef"], "fox-result://run-1/call-9");
        // Small content is returned untouched, not merely re-serialized.
        let small = json!({"ok": true, "value": "short"}).canonical_string();
        assert!(bounded_with_store("office_help", false, &small, None).is_none());
        // Incomplete identities have no reference.
        assert!(tool_result_ref("", "call").is_none());
        assert!(tool_result_ref("run", "").is_none());
    }

    #[test]
    fn generic_mcp_results_are_never_bounded() {
        // A generic call may describe a completed write: "call it again" is not
        // a safe read, so the result must pass through verbatim.
        let write_result = json!([{"type":"text","text": json!({
            "operation": "created", "id": "example-only", "body": "x".repeat(12_000)
        }).canonical_string()}]);
        assert!(bound_tool_result_content("call_mcp_tool", false, &write_result, None).is_none());
        // The tool listing itself is read-only reference material, but it is only
        // bounded while Host keeps the whole of it — omitted entries have to stay
        // reachable, otherwise the view would be dropping tools the model can
        // neither see nor recover.
        let listing = |tools: usize, description: usize| {
            json!([{"type":"text","text": json!({"tools": (0..tools)
                .map(|index| json!({"name": format!("mcp_{index}"), "description": "y".repeat(description)}))
                .collect::<Vec<_>>()}).canonical_string()}])
        };
        // With the Host confirming it stored the whole listing, the view may be
        // bounded and the note must say the reference actually reaches it.
        let small = listing(60, 200);
        let small_text = small[0]["text"].as_str().unwrap().to_owned();
        let bounded = bound_tool_result_content_with_storage(
            "list_mcp_tools",
            false,
            &small,
            Some(TEST_REF),
            &stored_whole(small_text.len()),
        )
        .unwrap();
        let projected: Value = serde_json::from_str(bounded[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            projected["foxModelView"]["retrievable"], json!(true),
            "a result Host stored whole is recoverable, so the view may omit"
        );
        assert!(
            projected["tools"].as_array().unwrap().len() <= 60,
            "the projection stays valid JSON with a subset of entries"
        );
        // Without that fact nothing may be dropped or promised: the view may
        // still shrink long field text, but every entry stays and the note must
        // not tell the model it can read omitted bytes back.
        let unverified = match bound_tool_result_content("list_mcp_tools", false, &small, None) {
            Some(view) => serde_json::from_str::<Value>(view[0]["text"].as_str().unwrap()).unwrap(),
            None => serde_json::from_str::<Value>(small[0]["text"].as_str().unwrap()).unwrap(),
        };
        assert_eq!(unverified["tools"].as_array().unwrap().len(), 60,
            "no entry may be dropped without a trusted storage fact");
        if unverified.get(STRUCT_VIEW_KEY).is_some() {
            assert_eq!(unverified[STRUCT_VIEW_KEY]["retrievable"], json!(false));
            assert_eq!(unverified[STRUCT_VIEW_KEY]["storageVerified"], json!(false));
        }
        assert!(small_text.len() > TOOL_VIEW_MAX_BYTES, "fixture must need a projection");
        // A partial store (preview only) is equally unusable, whatever the size.
        let huge = listing(300, 400);
        let huge_text = huge[0]["text"].as_str().unwrap().to_owned();
        let preview_only = match bound_tool_result_content_with_storage(
            "list_mcp_tools",
            false,
            &huge,
            None,
            &ToolResultStorage {
                stored: true,
                stored_bytes: 131_072,
                retrievable_bytes: 131_072,
                ..Default::default()
            },
        ) {
            Some(view) => serde_json::from_str::<Value>(view[0]["text"].as_str().unwrap()).unwrap(),
            None => serde_json::from_str::<Value>(&huge_text).unwrap(),
        };
        assert_eq!(preview_only["tools"].as_array().unwrap().len(), 300,
            "a preview-only store may not drop entries");
        if preview_only.get(STRUCT_VIEW_KEY).is_some() {
            assert_eq!(preview_only[STRUCT_VIEW_KEY]["retrievable"], json!(false));
        }
    }

    #[test]
    fn oversized_drop_in_a_paged_catalog_resumes_at_the_first_omitted_property() {
        let total = 400usize;
        let catalog = json!({
            "format":"xlsx","element":"chart","page":1,"pageSize":60,"totalProperties":total,
            "properties": (0..60).map(|index| json!({
                "name": format!("property{index:03}"), "type":"string", "ops":["set"],
                "hint": "中".repeat(160),
                // Numeric bulk that no string-leaf shrink can reduce: without it
                // step 1 always fits and the record-dropping path is never
                // reached, so the test would assert nothing about continuations.
                "tagSamples": (0..20).map(|_| json!(123_456)).collect::<Vec<_>>()
            })).collect::<Vec<_>>(),
            "complete": false,
            "next": {"tool":"office_help",
                "arguments":{"format":"xlsx","element":"chart","page":2,"pageSize":60},
                "instruction":"page 1/7"},
        });
        let text = catalog.canonical_string();
        assert!(text.len() > TOOL_VIEW_MAX_BYTES, "fixture must need a projection");
        let bounded = bounded_with_store("office_help", false, &text, Some(TEST_REF)).unwrap();
        assert!(bounded.len() <= TOOL_VIEW_MAX_BYTES, "projection must stay in budget");
        let parsed: Value = serde_json::from_str(&bounded).expect("bounded catalog is valid JSON");
        let kept: Vec<String> = parsed["properties"]
            .as_array()
            .expect("properties survive as an array")
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_string())
            .collect();
        assert!(kept.len() < 60, "the fixture must actually drop records");
        assert_eq!(parsed["next"]["tool"], "office_help");

        // Real paging contract of office.rs::shape_help: start = (page-1)*pageSize.
        let resume_page = parsed["next"]["arguments"]["page"].as_u64().unwrap();
        let resume_page_size = parsed["next"]["arguments"]["pageSize"].as_u64().unwrap();
        assert!((1..=60).contains(&resume_page_size), "pageSize must satisfy the tool schema");
        assert!(resume_page >= 1);
        let resume_start = (resume_page - 1) * resume_page_size;
        assert_eq!(
            resume_start,
            kept.len() as u64,
            "the continuation must start exactly at the first omitted record"
        );
        // Walking the real formula from the continuation reaches every omitted
        // property: nothing the projection dropped is unreachable. The returned
        // `pageSize` is kept — changing page size mid-walk would create gaps
        // that belong to the walk, not to the continuation.
        let names: Vec<String> = (0..total).map(|index| format!("property{index:03}")).collect();
        let mut reachable = std::collections::BTreeSet::new();
        let page_size = resume_page_size;
        let mut page = resume_page;
        while (page - 1) * page_size < total as u64 {
            let start = ((page - 1) * page_size) as usize;
            for name in names.iter().skip(start).take(page_size as usize) {
                reachable.insert(name.clone());
            }
            page += 1;
            if page > 800 {
                break;
            }
        }
        for name in names.iter().skip(kept.len()) {
            assert!(
                reachable.contains(name),
                "omitted record {name} must be reachable from the returned continuation"
            );
        }
    }

    #[test]
    fn navigation_survives_the_projection_or_the_result_is_left_unbounded() {
        // Structured path: navigation stays verbatim even when records are dropped.
        let catalog = office_catalog();
        let bounded = bounded_with_store("office_help", false, &catalog.canonical_string(), Some(TEST_REF)).unwrap();
        let parsed: Value = serde_json::from_str(&bounded).unwrap();
        assert_eq!(parsed["next"]["tool"], "office_help");

        // Two shapes whose navigation cannot be carried inside the budget: the
        // first has a `next` larger than the entire view, the second pages a
        // collection (`rows`) that is not the one paged tool this module knows
        // how to resume. Both must come back byte-identical. A bounded view that
        // omitted records it cannot address would claim reachability it does not
        // have, and a head/tail cut would publish invalid JSON.
        let selector = "s".repeat(12_000);
        let reviewed_paging_shape = json!({
            "rows": (0..220).map(|index| json!({
                "id": index + 1, "a": 123_456, "b": 123_456, "c": 123_456,
                "d": 123_456, "e": 123_456, "f": 123_456, "g": 123_456
            })).collect::<Vec<_>>(),
            "next": {"tool": "office_read", "arguments": {"offset": 220, "limit": 220}},
        });
        for payload in [
            json!({"rows":[{"id":1}], "next":{"tool":"office_read","arguments":{"selector":selector}}}),
            reviewed_paging_shape,
        ] {
            let text = payload.canonical_string();
            assert!(text.len() > TOOL_VIEW_MAX_BYTES, "fixture must be over the view budget");
            // No trusted storage fact here: the payload must come back whole.
            let out = bound_tool_text("office_read", false, &text, None, &ToolResultStorage::unknown())
                .unwrap();
            assert_eq!(out, text, "an unaddressable payload must be returned unchanged");
            let reparsed: Value = serde_json::from_str(&out).expect("still valid JSON");
            assert_eq!(reparsed, payload, "no record and no navigation entry may be lost");
        }
    }

}
