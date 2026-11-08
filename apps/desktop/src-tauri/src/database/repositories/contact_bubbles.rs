//! Durable contact bubble projection (plan contracts C00 and K1).
//!
//! A bubble is written once, when its source message is already terminal, and
//! every later read uses the stored identity. Identity, projection mode,
//! segment range and delivery order never change after the row exists; only
//! `content` and `fingerprint` may be recomputed from the message and events.
//!
//! The projection rules mirror the TypeScript `projectRuntimeGroups` (delta
//! segmentation) and `projectKernelGroups` (Kernel checkpoint responses). When
//! neither can be verified against the committed content, the message becomes
//! one bubble whose identity is the message id.
//!
//! Product decision (K1): a user message with no text (attachment only) gets no
//! bubble. There is nothing to show or quote as text; later features (quote,
//! favorite) address such messages by message id. A visible attachment bubble
//! would need its own design and is deliberately out of scope.

use super::now_ms;
use crate::database::{ContactBubbleRecord, ContactBubbleSearchHit, ContactBubbleSearchPage};
use rusqlite::{params, Connection};
use serde_json::Value;

/// A segment before it becomes a durable row.
#[derive(Debug, Clone, PartialEq)]
struct Segment {
    start: i64,
    end: i64,
    text: String,
    /// Time the segment's first delta arrived, not the end of the whole run.
    delivered_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Projection {
    Message,
    Segmented,
    Kernel,
}

impl Projection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Segmented => "segmented",
            Self::Kernel => "kernel",
        }
    }
}

#[derive(Debug)]
struct SourceMessage {
    id: String,
    run_id: Option<String>,
    role: String,
    content: String,
    ordinal: i64,
    updated_at: i64,
}

pub(crate) fn fingerprint(text: &str) -> String {
    let hash = text.as_bytes().iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

/// Mirrors `splitAssistantContent`: inline reasoning never becomes bubble text.
pub(crate) fn visible_answer(content: &str) -> String {
    const TAGS: [&str; 4] = ["think", "thinking", "analysis", "mm:think"];
    let lower = content.to_ascii_lowercase();
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0usize;
    while cursor < content.len() {
        let rest = &lower[cursor..];
        let Some(open) = rest.find('<') else { break };
        let open_at = cursor + open;
        if open > 0 {
            out.push_str(&content[cursor..open_at]);
        }
        let Some(close) = lower[open_at..].find('>').map(|offset| open_at + offset + 1) else {
            // Not a tag we understand: keep the byte and advance.
            out.push_str(&content[open_at..open_at + 1]);
            cursor = open_at + 1;
            continue;
        };
        let tag = &lower[open_at + 1..close - 1];
        if let Some(name) = tag.strip_prefix('/') {
            if TAGS.contains(&name) {
                cursor = close;
                continue;
            }
        } else if TAGS.contains(&tag) {
            let closing = format!("</{tag}>");
            if let Some(relative) = lower[close..].find(&closing) {
                cursor = close + relative + closing.len();
            } else {
                cursor = content.len();
            }
            continue;
        }
        out.push_str(&content[open_at..close]);
        cursor = close;
    }
    if cursor < content.len() {
        out.push_str(&content[cursor..]);
    }
    out.replace('\u{feff}', "").trim().to_owned()
}

fn process_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "reasoning.delta" | "tool.started" | "tool.updated" | "tool.completed"
    )
}

/// Events that end the current response segment in `projectRuntimeGroups`.
///
/// A retry is two responses, not one: the abandoned attempt and the retried
/// attempt are separate messages to the user, so `run.retrying`,
/// `run.retry.completed` and the `request_sent` phase that follows a retry all
/// split. Missing them would join text the live view already shows apart.
fn boundary_event(event_type: &str, payload: &Value, retried: bool) -> bool {
    matches!(
        event_type,
        "user.question.requested"
            | "user.question.responded"
            | "run.failed"
            | "run.interrupted"
            | "run.cancelled"
            | "run.retrying"
            | "run.retry.completed"
    ) || (event_type == "run.phase"
        && retried
        && payload.get("phase").and_then(Value::as_str) == Some("request_sent"))
}

/// Port of `projectRuntimeGroups`, verified against the committed content.
fn delta_segments(
    content: &str,
    events: &[(i64, String, Value, i64)],
) -> Option<Vec<Segment>> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut current: Option<usize> = None;
    let mut pending = String::new();
    let mut assembled = String::new();
    let mut retried = false;
    for (seq, event_type, payload, created_at) in events {
        if boundary_event(event_type, payload, retried) || process_event(event_type) {
            current = None;
        }
        if event_type == "run.retrying" {
            retried = true;
        }
        if event_type == "message.delta" {
            let delta = payload.get("delta").and_then(Value::as_str).unwrap_or_default();
            assembled.push_str(delta);
            if delta.is_empty() {
                continue;
            }
            if let Some(index) = current {
                segments[index].text.push_str(delta);
                segments[index].end = *seq;
            } else if delta.trim().is_empty() {
                // Segment-leading whitespace joins the next reply and never
                // decides identity.
                pending.push_str(delta);
            } else {
                let text = format!("{pending}{delta}");
                pending.clear();
                segments.push(Segment { start: *seq, end: *seq, text, delivered_at: *created_at });
                current = Some(segments.len() - 1);
            }
        }
    }
    if assembled != content {
        return None;
    }
    if !pending.is_empty() {
        match segments.last_mut() {
            Some(last) => last.text.push_str(&pending),
            None => return None,
        }
    }
    segments.retain(|segment| !segment.text.trim().is_empty());
    (!segments.is_empty()).then_some(segments)
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(chunk) = part.get("text").and_then(Value::as_str) {
                        text.push_str(chunk);
                    }
                }
            }
            Some(text)
        }
        _ => None,
    }
}

/// Kernel checkpoint responses, verified against the committed content.
fn kernel_segments(connection: &Connection, run_id: &str, content: &str) -> Option<Vec<Segment>> {
    let mut statement = connection
        .prepare(
            "SELECT CAST(json_extract(k.payload_json,'$.response.checkpointSeq') AS INTEGER),
                    json_extract(k.payload_json,'$.response.assistantMessage.content'),
                    k.created_at
             FROM kernel_events k
             JOIN run_control_bindings c ON c.run_id = k.run_id AND c.authority = 'authoritative'
             WHERE k.run_id = ?1
               AND k.event_type IN ('engine.initial_response','engine.batch_response','engine.continuation_response')
             ORDER BY 1 ASC",
        )
        .ok()?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .ok()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .ok()?;
    let mut segments = Vec::new();
    let mut assembled = String::new();
    for (checkpoint, raw, created_at) in rows {
        let checkpoint = checkpoint?;
        let text = raw
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| json_text(&value))
            .unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        assembled.push_str(&text);
        segments.push(Segment { start: checkpoint, end: checkpoint, text, delivered_at: created_at });
    }
    (assembled == content && !segments.is_empty()).then_some(segments)
}

fn run_events(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Vec<(i64, String, Value, i64)>> {
    let mut statement = connection.prepare(
        "SELECT seq, event_type, event_json, created_at FROM run_events WHERE run_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = statement
        .query_map([run_id], |row| {
            let raw: String = row.get(2)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null),
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect();
    rows
}

fn insert_bubble(
    connection: &Connection,
    conversation_id: &str,
    source: &SourceMessage,
    author: &str,
    projection: Projection,
    id: &str,
    segment: Option<(i64, i64)>,
    index: usize,
    text: &str,
    delivered_at: i64,
) -> rusqlite::Result<()> {
    let now = now_ms();
    connection.execute(
        "INSERT OR IGNORE INTO contact_bubbles(
            bubble_id, conversation_id, run_id, source_message_id, source_ordinal,
            author, projection, segment_start, segment_end, delivery_order,
            delivered_at, content, fingerprint, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            id,
            conversation_id,
            source.run_id.as_deref(),
            source.id,
            source.ordinal,
            author,
            projection.as_str(),
            segment.map(|(start, _)| start),
            segment.map(|(_, end)| end),
            source.ordinal.saturating_mul(1000).saturating_add(index as i64),
            delivered_at,
            text,
            fingerprint(text),
            now,
        ],
    )?;
    Ok(())
}

fn project_assistant(
    connection: &Connection,
    conversation_id: &str,
    source: &SourceMessage,
) -> rusqlite::Result<()> {
    let Some(run_id) = source.run_id.as_deref() else {
        let answer = visible_answer(&source.content);
        if answer.is_empty() {
            return Ok(());
        }
        return insert_bubble(connection, conversation_id, source, "companion", Projection::Message, &source.id, None, 0, &answer, source.updated_at);
    };
    let existing: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM contact_bubbles WHERE conversation_id = ?1 AND source_message_id = ?2)",
        params![conversation_id, source.id],
        |row| row.get(0),
    )?;
    if existing {
        return Ok(());
    }
    let raw = source.content.as_str();
    let events = run_events(connection, run_id)?;
    let kernel = kernel_segments(connection, run_id, raw);
    let segmented = kernel.is_none().then(|| delta_segments(raw, &events)).flatten();
    if let Some(segments) = kernel {
        for (index, segment) in segments.into_iter().enumerate() {
            let text = visible_answer(&segment.text);
            if text.is_empty() {
                continue;
            }
            let id = format!("{run_id}:kernel:{}:response", segment.start);
            insert_bubble(connection, conversation_id, source, "companion", Projection::Kernel, &id, Some((segment.start, segment.end)), index, &text, segment.delivered_at)?;
        }
        return Ok(());
    }
    if let Some(segments) = segmented {
        for (index, segment) in segments.into_iter().enumerate() {
            let text = visible_answer(&segment.text);
            if text.is_empty() {
                continue;
            }
            let id = format!("{run_id}:response:{}", segment.start);
            insert_bubble(connection, conversation_id, source, "companion", Projection::Segmented, &id, Some((segment.start, segment.end)), index, &text, segment.delivered_at)?;
        }
        return Ok(());
    }
    let answer = visible_answer(raw);
    if answer.is_empty() {
        return Ok(());
    }
    insert_bubble(connection, conversation_id, source, "companion", Projection::Message, &source.id, None, 0, &answer, source.updated_at)
}

fn is_companion_chat(connection: &Connection, conversation_id: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1 AND companion_id IS NOT NULL)",
        [conversation_id],
        |row| row.get(0),
    )
}

fn unprojected_messages(
    connection: &Connection,
    conversation_id: &str,
    from_ordinal: i64,
    before_ordinal: Option<i64>,
) -> rusqlite::Result<Vec<SourceMessage>> {
    let mut statement = connection.prepare(
        "SELECT m.id, m.run_id, m.role, m.content, m.ordinal, m.updated_at
         FROM messages m
         WHERE m.conversation_id = ?1
           AND m.ordinal >= ?2
           AND (?3 IS NULL OR m.ordinal < ?3)
           AND m.role IN ('user', 'assistant')
           AND m.status NOT IN ('streaming', 'superseded')
           AND NOT EXISTS(
             SELECT 1 FROM contact_bubbles b
             WHERE b.conversation_id = m.conversation_id AND b.source_message_id = m.id
           )
         ORDER BY m.ordinal ASC",
    )?;
    let rows = statement
        .query_map(params![conversation_id, from_ordinal, before_ordinal], |row| {
            Ok(SourceMessage {
                id: row.get(0)?,
                run_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                ordinal: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?
        .collect();
    rows
}

/// One message becomes all of its bubbles or none. A half-written message would
/// leave the early-exit check below satisfied and silently drop the remaining
/// segments forever.
fn project_source(
    connection: &Connection,
    conversation_id: &str,
    message: &SourceMessage,
) -> rusqlite::Result<()> {
    if message.role == "user" {
        // A text-less message (attachment only) has no bubble to show or quote;
        // later features address it by message id instead of a bubble id.
        if message.content.trim().is_empty() {
            return Ok(());
        }
        let existing: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM contact_bubbles WHERE conversation_id = ?1 AND source_message_id = ?2)",
            params![conversation_id, message.id],
            |row| row.get(0),
        )?;
        if existing {
            return Ok(());
        }
        return insert_bubble(connection, conversation_id, message, "user", Projection::Message, &message.id, None, 0, &message.content, message.updated_at);
    }
    project_assistant(connection, conversation_id, message)
}

fn load_source_message(
    connection: &Connection,
    message_id: &str,
) -> rusqlite::Result<Option<SourceMessage>> {
    let mut statement = connection.prepare(
        "SELECT id, run_id, role, content, ordinal, updated_at
         FROM messages
         WHERE id = ?1 AND status NOT IN ('streaming', 'superseded')",
    )?;
    let mut rows = statement.query_map([message_id], |row| {
        Ok(SourceMessage {
            id: row.get(0)?,
            run_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            ordinal: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })?;
    rows.next().transpose()
}

/// Projects one message inside the caller's transaction, so a bubble becomes
/// visible exactly when the message reaches a terminal state.
pub(crate) fn project_message_in_transaction(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> rusqlite::Result<()> {
    if !is_companion_chat(connection, conversation_id)? {
        return Ok(());
    }
    let Some(message) = load_source_message(connection, message_id)? else {
        return Ok(());
    };
    project_source(connection, conversation_id, &message)
}

/// Projects the assistant message a Run ended with, inside the caller's
/// transaction. Called from every terminal Run transition.
pub(crate) fn project_run_in_transaction(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<()> {
    let Some(conversation_id) = connection
        .query_row("SELECT conversation_id FROM runs WHERE id = ?1", [run_id], |row| {
            row.get::<_, String>(0)
        })
        .ok()
    else {
        return Ok(());
    };
    if !is_companion_chat(connection, &conversation_id)? {
        return Ok(());
    }
    let message_id: Option<String> = connection.query_row(
        "SELECT id FROM messages
         WHERE run_id = ?1 AND role = 'assistant' AND status NOT IN ('streaming', 'superseded')
         ORDER BY ordinal DESC LIMIT 1",
        [run_id],
        |row| row.get(0),
    ).ok();
    let Some(message_id) = message_id else {
        return Ok(());
    };
    project_message_in_transaction(connection, &conversation_id, &message_id)
}

/// Projects every assistant message a startup repair just interrupted, inside
/// the repair transaction. One query finds them; individual projections are
/// cheap because they only touch contact conversations.
pub(crate) fn project_interrupted_in_transaction(connection: &Connection) -> rusqlite::Result<()> {
    let mut statement = connection.prepare(
        "SELECT m.conversation_id, m.id
         FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.status = 'interrupted'
           AND m.role = 'assistant'
           AND c.companion_id IS NOT NULL
           AND NOT EXISTS(
             SELECT 1 FROM contact_bubbles b
             WHERE b.conversation_id = m.conversation_id AND b.source_message_id = m.id
           )",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (conversation_id, message_id) in rows {
        project_message_in_transaction(connection, &conversation_id, &message_id)?;
    }
    Ok(())
}

/// `true` once every message of the private chat has bubbles, so permanent
/// operations (quote, favorite, mark unread) can rely on bubble identity.
pub(crate) fn backfill_complete(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM contact_bubble_backfills WHERE conversation_id = ?1)",
        [conversation_id],
        |row| row.get(0),
    )
}

/// Write the missing bubbles for one conversation window. Idempotent: a row
/// that already exists is never rewritten, so identities survive restart and
/// pagination. Only contacts get bubbles; ordinary conversations are skipped.
pub(crate) fn ensure_window(
    connection: &Connection,
    conversation_id: &str,
    from_ordinal: i64,
    before_ordinal: Option<i64>,
) -> rusqlite::Result<()> {
    if !is_companion_chat(connection, conversation_id)? {
        return Ok(());
    }
    for message in unprojected_messages(connection, conversation_id, from_ordinal, before_ordinal)? {
        let transaction = connection.unchecked_transaction()?;
        project_source(&transaction, conversation_id, &message)?;
        transaction.commit()?;
    }
    Ok(())
}

pub(crate) fn query_window(
    connection: &Connection,
    conversation_id: &str,
    from_ordinal: i64,
    before_ordinal: Option<i64>,
) -> rusqlite::Result<Vec<ContactBubbleRecord>> {
    let mut statement = connection.prepare(
        "SELECT bubble_id, conversation_id, run_id, source_message_id, source_ordinal,
                author, projection, segment_start, segment_end, delivery_order,
                delivered_at, content, fingerprint
         FROM contact_bubbles
         WHERE conversation_id = ?1
           AND source_ordinal >= ?2
           AND (?3 IS NULL OR source_ordinal < ?3)
         ORDER BY delivery_order ASC",
    )?;
    let rows = statement
        .query_map(params![conversation_id, from_ordinal, before_ordinal], |row| {
            Ok(ContactBubbleRecord {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                run_id: row.get(2)?,
                message_id: row.get(3)?,
                source_ordinal: row.get(4)?,
                author: row.get(5)?,
                projection: row.get(6)?,
                segment_start: row.get(7)?,
                segment_end: row.get(8)?,
                delivery_order: row.get(9)?,
                delivered_at: row.get(10)?,
                content: row.get(11)?,
                fingerprint: row.get(12)?,
            })
        })?
        .collect();
    rows
}

/// C01: literal substring search over one private chat's committed bubbles.
/// `%` and `_` in the query are escaped so they match themselves; LIKE folds
/// ASCII case only, so Chinese matches literally without segmentation. Results
/// are newest first and carry the stable identity plus source ordinal used to
/// page history back to the bubble. Ordinary conversations return nothing.
pub(crate) fn search_bubbles(
    connection: &Connection,
    conversation_id: &str,
    query: &str,
    since: Option<i64>,
    until: Option<i64>,
    limit: usize,
    offset: usize,
) -> rusqlite::Result<ContactBubbleSearchPage> {
    let query = query.trim();
    let empty = ContactBubbleSearchPage { total: 0, results: Vec::new() };
    if query.is_empty() || query.chars().count() > 200 || !is_companion_chat(connection, conversation_id)? {
        return Ok(empty);
    }
    let limit = limit.clamp(1, 100) as i64;
    let offset = offset.min(10_000) as i64;
    let pattern = format!("%{}%", query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let total: i64 = connection.query_row(
        "SELECT COUNT(*) FROM contact_bubbles
         WHERE conversation_id = ?1 AND content LIKE ?2 ESCAPE '\\'
           AND (?3 IS NULL OR delivered_at >= ?3)
           AND (?4 IS NULL OR delivered_at <= ?4)",
        params![conversation_id, pattern, since, until],
        |row| row.get(0),
    )?;
    let mut statement = connection.prepare(
        "SELECT bubble_id, author, content, delivered_at, source_ordinal, run_id, source_message_id
         FROM contact_bubbles
         WHERE conversation_id = ?1 AND content LIKE ?2 ESCAPE '\\'
           AND (?3 IS NULL OR delivered_at >= ?3)
           AND (?4 IS NULL OR delivered_at <= ?4)
         ORDER BY delivery_order DESC
         LIMIT ?5 OFFSET ?6",
    )?;
    let results = statement
        .query_map(params![conversation_id, pattern, since, until, limit, offset], |row| {
            let content: String = row.get(2)?;
            let (snippet, match_start, match_end) = search_snippet(&content, query);
            Ok(ContactBubbleSearchHit {
                id: row.get(0)?,
                author: row.get(1)?,
                snippet,
                match_start,
                match_end,
                delivered_at: row.get(3)?,
                source_ordinal: row.get(4)?,
                run_id: row.get(5)?,
                message_id: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ContactBubbleSearchPage { total: total as usize, results })
}

/// A window around the first ASCII-case-insensitive match (the same folding
/// SQLite LIKE applies), with char offsets of the match inside the snippet.
fn search_snippet(content: &str, query: &str) -> (String, usize, usize) {
    let haystack: Vec<char> = content.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    let position = if needle.is_empty() || haystack.len() < needle.len() {
        None
    } else {
        (0..=haystack.len() - needle.len()).find(|&start| {
            haystack[start..start + needle.len()]
                .iter()
                .zip(&needle)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
    };
    let Some(position) = position else {
        return (haystack.iter().take(96).collect(), 0, 0);
    };
    let start = position.saturating_sub(32);
    let end = (position + needle.len() + 64).min(haystack.len());
    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.extend(&haystack[start..end]);
    if end < haystack.len() {
        snippet.push('…');
    }
    let lead = usize::from(start > 0);
    (snippet, position - start + lead, position - start + lead + needle.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn event(seq: i64, event_type: &str, payload: Value, created_at: i64) -> (i64, String, Value, i64) {
        (seq, event_type.to_owned(), payload, created_at)
    }

    fn delta(seq: i64, text: &str, created_at: i64) -> (i64, String, Value, i64) {
        event(seq, "message.delta", serde_json::json!({ "type": "message.delta", "delta": text }), created_at)
    }

    #[test]
    fn leading_whitespace_does_not_decide_identity() {
        let content = "\n第一条\n第二条";
        let segments = delta_segments(content, &[delta(1, "\n", 100), delta(2, "第一条\n", 200), delta(3, "第二条", 300)])
            .expect("segments");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start, 2);
        assert_eq!(segments[0].text, content);
    }

    #[test]
    fn tools_split_responses_and_keep_whitespace_between_rounds() {
        let content = "第一段\n第二段";
        let segments = delta_segments(
            content,
            &[
                event(1, "message.delta", serde_json::json!({"delta": "第一段"}), 100),
                event(2, "tool.started", serde_json::json!({"toolCallId": "t"}), 200),
                event(3, "message.delta", serde_json::json!({"delta": "\n"}), 300),
                event(4, "message.delta", serde_json::json!({"delta": "第二段"}), 400),
            ],
        )
        .expect("segments");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].start, 1);
        assert_eq!(segments[1].start, 4);
        assert_eq!(segments[1].text, "\n第二段");
    }

    #[test]
    fn coverage_mismatch_falls_back_to_one_message_bubble() {
        assert!(delta_segments("别的正文", &[delta(1, "正文", 100)]).is_none());
        assert!(delta_segments("正文", &[]).is_none());
    }

    #[test]
    fn trailing_whitespace_joins_the_last_response() {
        let segments = delta_segments("正文\n", &[delta(1, "正文", 100), delta(2, "\n", 200)]).expect("segments");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "正文\n");
    }

    #[test]
    fn retry_splits_the_abandoned_attempt_from_the_new_one() {
        // Live view: run.retrying, then the request_sent phase that follows it,
        // both end the current segment. The abandoned text must never swallow
        // the retried reply, and the retried reply keeps its own identity.
        let content = "废弃的回答正确的回答";
        let segments = delta_segments(
            content,
            &[
                delta(1, "废弃的回答", 100),
                event(2, "run.retrying", serde_json::json!({"attempt": 1}), 200),
                event(3, "run.phase", serde_json::json!({"phase": "request_sent"}), 300),
                delta(4, "正确的回答", 400),
            ],
        )
        .expect("segments");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].start, 1);
        assert_eq!(segments[0].end, 1);
        assert_eq!(segments[0].text, "废弃的回答");
        assert_eq!(segments[1].start, 4);
        assert_eq!(segments[1].text, "正确的回答");
        assert_eq!(segments[1].delivered_at, 400);
    }

    #[test]
    fn retry_completed_also_splits_and_request_sent_without_retry_does_not() {
        let content = "第一段第二段";
        let segments = delta_segments(
            content,
            &[
                delta(1, "第一段", 100),
                event(2, "run.phase", serde_json::json!({"phase": "request_sent"}), 200),
                delta(3, "第二段", 300),
            ],
        )
        .expect("segments");
        assert_eq!(segments.len(), 1, "request_sent alone is not a boundary");
        let segments = delta_segments(
            content,
            &[
                delta(1, "第一段", 100),
                event(2, "run.retry.completed", serde_json::json!({}), 200),
                delta(3, "第二段", 300),
            ],
        )
        .expect("segments");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].start, 3);
    }

    #[test]
    fn delivered_at_is_the_first_delta_of_the_segment_not_the_run_end() {
        let content = "\n第一段\n第二段";
        let segments = delta_segments(
            content,
            &[
                delta(1, "\n", 100),
                delta(2, "第一段", 200),
                event(4, "tool.completed", serde_json::json!({"toolCallId": "t"}), 400),
                delta(5, "\n", 500),
                delta(6, "第二段", 600),
                delta(7, "", 700),
                event(8, "run.completed", serde_json::json!({}), 999),
            ],
        )
        .expect("segments");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].delivered_at, 200, "leading whitespace at seq 1 does not own the time");
        assert_eq!(segments[1].delivered_at, 600);
    }

    #[test]
    fn inline_reasoning_never_reaches_the_bubble() {
        assert_eq!(visible_answer("<think>内部</think>正文"), "正文");
        assert_eq!(visible_answer("<thinking>内部"), "");
        assert_eq!(visible_answer("正文</analysis>"), "正文");
    }

    #[test]
    fn a_failed_projection_leaves_no_half_written_message() {
        // project_assistant skips a message as soon as ANY row exists. If a
        // multi-segment write could fail halfway, the remaining segments would
        // be lost forever. The per-message transaction must roll back instead.
        let connection = Connection::open_in_memory().expect("open");
        connection.execute_batch(
            "CREATE TABLE conversations(id TEXT PRIMARY KEY, companion_id TEXT);
             CREATE TABLE messages(id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, role TEXT,
                 content TEXT, status TEXT, ordinal INTEGER, updated_at INTEGER);
             CREATE TABLE contact_bubbles(
                 bubble_id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, source_message_id TEXT,
                 source_ordinal INTEGER, author TEXT, projection TEXT, segment_start INTEGER,
                 segment_end INTEGER, delivery_order INTEGER, delivered_at INTEGER,
                 content TEXT, fingerprint TEXT, created_at INTEGER);
             CREATE TABLE run_events(run_id TEXT, seq INTEGER, event_type TEXT, event_json TEXT, created_at INTEGER);
             INSERT INTO conversations VALUES('chat', 'cmp_contact');
             INSERT INTO messages VALUES('assistant-run', 'chat', 'run', 'assistant', '第一段第二段', 'completed', 1, 900);
             INSERT INTO run_events VALUES('run', 1, 'message.delta', '{\"delta\":\"第一段\"}', 100);
             INSERT INTO run_events VALUES('run', 2, 'tool.started', '{\"toolCallId\":\"t\"}', 200);
             INSERT INTO run_events VALUES('run', 3, 'message.delta', '{\"delta\":\"第二段\"}', 300);
             CREATE TRIGGER fail_second_segment BEFORE INSERT ON contact_bubbles
                 WHEN NEW.segment_start = 3
                 BEGIN SELECT RAISE(ABORT, 'injected mid-message failure'); END;",
        )
        .expect("seed");
        assert!(ensure_window(&connection, "chat", 0, None).is_err());
        let rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM contact_bubbles WHERE source_message_id = 'assistant-run'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(rows, 0, "the first segment must roll back with the failed one");
        connection.execute_batch("DROP TRIGGER fail_second_segment;").expect("drop");
        ensure_window(&connection, "chat", 0, None).expect("retry after rollback");
        let bubbles = query_window(&connection, "chat", 0, None).expect("read");
        assert_eq!(bubbles.len(), 2);
        assert_eq!(bubbles[0].id, "run:response:1");
        assert_eq!(bubbles[1].id, "run:response:3");
    }

    #[test]
    fn attachment_only_user_message_gets_no_bubble() {
        let connection = Connection::open_in_memory().expect("open");
        connection.execute_batch(
            "CREATE TABLE conversations(id TEXT PRIMARY KEY, companion_id TEXT);
             CREATE TABLE messages(id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, role TEXT,
                 content TEXT, status TEXT, ordinal INTEGER, updated_at INTEGER);
             CREATE TABLE contact_bubbles(
                 bubble_id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, source_message_id TEXT,
                 source_ordinal INTEGER, author TEXT, projection TEXT, segment_start INTEGER,
                 segment_end INTEGER, delivery_order INTEGER, delivered_at INTEGER,
                 content TEXT, fingerprint TEXT, created_at INTEGER);
             INSERT INTO conversations VALUES('chat', 'cmp_contact');
             INSERT INTO messages VALUES('user-files', 'chat', NULL, 'user', '  ', 'completed', 1, 10);",
        )
        .expect("seed");
        ensure_window(&connection, "chat", 0, None).expect("project");
        assert!(query_window(&connection, "chat", 0, None).expect("read").is_empty());
    }

    #[test]
    fn shared_fixture_matches_the_typescript_projection() {
        // The same JSON drives apps/desktop/tests/contact-bubbles-fixture.test.ts;
        // ids and visible text must agree one by one.
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Case {
            name: String,
            mode: String,
            run_id: String,
            content: String,
            events: Vec<Event>,
            kernel_checkpoints: Option<Vec<Checkpoint>>,
            expected: Vec<Bubble>,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Event {
            seq: i64,
            event_type: String,
            payload: Value,
            created_at: i64,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Checkpoint {
            checkpoint_seq: i64,
            content: String,
            created_at: i64,
        }
        #[derive(serde::Deserialize)]
        struct Bubble {
            id: String,
            text: String,
        }
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/contact-bubbles.json"
        ))
        .expect("fixture parses");
        let cases: Vec<Case> = serde_json::from_value(fixture["cases"].clone()).expect("cases");
        assert!(cases.len() >= 6);
        for case in cases {
            let connection = Connection::open_in_memory().expect("open");
            connection.execute_batch(
                "CREATE TABLE conversations(id TEXT PRIMARY KEY, companion_id TEXT);
                 CREATE TABLE messages(id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, role TEXT,
                     content TEXT, status TEXT, ordinal INTEGER, updated_at INTEGER);
                 CREATE TABLE contact_bubbles(
                     bubble_id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, source_message_id TEXT,
                     source_ordinal INTEGER, author TEXT, projection TEXT, segment_start INTEGER,
                     segment_end INTEGER, delivery_order INTEGER, delivered_at INTEGER,
                     content TEXT, fingerprint TEXT, created_at INTEGER);
                 CREATE TABLE run_events(run_id TEXT, seq INTEGER, event_type TEXT, event_json TEXT, created_at INTEGER);
                 CREATE TABLE kernel_events(run_id TEXT, seq INTEGER, event_type TEXT, payload_json TEXT, created_at INTEGER);
                 CREATE TABLE run_control_bindings(run_id TEXT, authority TEXT);
                 INSERT INTO conversations VALUES('chat', 'cmp_contact');",
            )
            .expect("schema");
            connection.execute(
                "INSERT INTO messages VALUES('assistant-run', 'chat', ?1, 'assistant', ?2, 'completed', 1, 999)",
                params![case.run_id, case.content],
            )
            .expect("message");
            for event in &case.events {
                let mut payload = event.payload.clone();
                payload["type"] = Value::String(event.event_type.clone());
                connection.execute(
                    "INSERT INTO run_events VALUES(?1, ?2, ?3, ?4, ?5)",
                    params![case.run_id, event.seq, event.event_type, payload.to_string(), event.created_at],
                )
                .expect("event");
            }
            for (index, checkpoint) in case.kernel_checkpoints.unwrap_or_default().iter().enumerate() {
                // Real kernel payloads carry content as a JSON block array;
                // json_extract returns it as a JSON string for re-parsing.
                let payload = serde_json::json!({
                    "response": {
                        "checkpointSeq": checkpoint.checkpoint_seq,
                        "assistantMessage": { "content": [{ "type": "text", "text": checkpoint.content }] },
                    }
                });
                connection.execute(
                    "INSERT INTO kernel_events VALUES(?1, ?2, 'engine.batch_response', ?3, ?4)",
                    params![case.run_id, index as i64 + 1, payload.to_string(), checkpoint.created_at],
                )
                .expect("kernel event");
            }
            connection.execute(
                "INSERT INTO run_control_bindings VALUES(?1, 'authoritative')",
                [&case.run_id],
            )
            .expect("binding");
            let source = load_source_message(&connection, "assistant-run")
                .expect("load")
                .expect("message exists");
            project_assistant(&connection, "chat", &source).expect("project");
            let bubbles = query_window(&connection, "chat", 0, None).expect("read");
            let actual: Vec<(String, String)> = bubbles
                .iter()
                .map(|bubble| (bubble.id.clone(), bubble.content.clone()))
                .collect();
            let expected: Vec<(String, String)> = case
                .expected
                .iter()
                .map(|bubble| (bubble.id.clone(), bubble.text.clone()))
                .collect();
            assert_eq!(actual, expected, "case '{}'", case.name);
            if case.mode == "kernel" {
                assert!(bubbles.iter().all(|bubble| bubble.projection == "kernel"), "case '{}'", case.name);
            } else {
                assert!(bubbles.iter().all(|bubble| bubble.projection == "segmented"), "case '{}' must fall back to delta segments", case.name);
            }
        }
    }

    #[test]
    fn projection_is_idempotent_and_never_renumbers() {
        let connection = Connection::open_in_memory().expect("open");
        connection.execute_batch(
            "CREATE TABLE conversations(id TEXT PRIMARY KEY, companion_id TEXT);
             CREATE TABLE messages(id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, role TEXT,
                 content TEXT, status TEXT, ordinal INTEGER, updated_at INTEGER);
             CREATE TABLE contact_bubbles(
                 bubble_id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, source_message_id TEXT,
                 source_ordinal INTEGER, author TEXT, projection TEXT, segment_start INTEGER,
                 segment_end INTEGER, delivery_order INTEGER, delivered_at INTEGER,
                 content TEXT, fingerprint TEXT, created_at INTEGER);
             CREATE TABLE run_events(run_id TEXT, seq INTEGER, event_type TEXT, event_json TEXT, created_at INTEGER);
             INSERT INTO conversations VALUES('chat', 'cmp_contact');
             INSERT INTO messages VALUES('user-1', 'chat', NULL, 'user', '问题', 'completed', 1, 10);
             INSERT INTO messages VALUES('assistant-run', 'chat', 'run', 'assistant', '回答', 'completed', 2, 20);
             INSERT INTO run_events VALUES('run', 1, 'message.delta', '{\"delta\":\"回答\"}', 15);",
        )
        .expect("seed");
        ensure_window(&connection, "chat", 0, None).expect("project");
        let first = query_window(&connection, "chat", 0, None).expect("read");
        assert_eq!(first.len(), 2);
        assert_eq!(first[1].id, "run:response:1");
        ensure_window(&connection, "chat", 0, None).expect("repeat");
        let second = query_window(&connection, "chat", 0, None).expect("read again");
        assert_eq!(first.iter().map(|bubble| &bubble.id).collect::<Vec<_>>(), second.iter().map(|bubble| &bubble.id).collect::<Vec<_>>());
        assert_eq!(second[1].content, "回答");
    }

    fn search_db() -> Connection {
        let connection = Connection::open_in_memory().expect("open");
        connection.execute_batch(
            "CREATE TABLE conversations(id TEXT PRIMARY KEY, companion_id TEXT);
             CREATE TABLE contact_bubbles(
                 bubble_id TEXT PRIMARY KEY, conversation_id TEXT, run_id TEXT, source_message_id TEXT,
                 source_ordinal INTEGER, author TEXT, projection TEXT, segment_start INTEGER,
                 segment_end INTEGER, delivery_order INTEGER, delivered_at INTEGER,
                 content TEXT, fingerprint TEXT, created_at INTEGER);
             INSERT INTO conversations VALUES('chat', 'cmp_contact');",
        )
        .expect("schema");
        connection
    }

    fn seed_bubble(connection: &Connection, id: &str, chat: &str, author: &str, order: i64, delivered_at: i64, ordinal: i64, content: &str) {
        connection.execute(
            "INSERT INTO contact_bubbles(bubble_id, conversation_id, run_id, source_message_id,
                 source_ordinal, author, projection, segment_start, segment_end,
                 delivery_order, delivered_at, content, fingerprint, created_at)
             VALUES(?1, ?2, 'run', ?3, ?4, ?5, 'segmented', NULL, NULL, ?6, ?7, ?8, '', 0)",
            params![id, chat, format!("msg-{id}"), ordinal, author, order, delivered_at, content],
        )
        .expect("bubble");
    }

    fn matched_text(hit: &ContactBubbleSearchHit) -> String {
        hit.snippet.chars().skip(hit.match_start).take(hit.match_end - hit.match_start).collect()
    }

    #[test]
    fn search_finds_chinese_substring_and_reports_snippet_offsets() {
        let connection = search_db();
        seed_bubble(&connection, "run:response:1", "chat", "companion", 1000, 500, 2, "整理好了。含税价最低的是华东机电，12,800 元。");
        seed_bubble(&connection, "user-1", "chat", "user", 0, 100, 1, "帮我把这三家供应商的报价整理成对比表。");
        let page = search_bubbles(&connection, "chat", "报价", None, None, 50, 0).expect("search");
        assert_eq!(page.total, 1);
        let hit = &page.results[0];
        assert_eq!(hit.id, "user-1");
        assert_eq!(hit.author, "user");
        assert_eq!(matched_text(hit), "报价");
        assert_eq!(hit.snippet, "帮我把这三家供应商的报价整理成对比表。");

        let long = format!("{}报价{}", "前".repeat(50), "后".repeat(80));
        seed_bubble(&connection, "long", "chat", "companion", 2000, 600, 3, &long);
        let page = search_bubbles(&connection, "chat", "报价", None, None, 50, 0).expect("search");
        let hit = page.results.iter().find(|hit| hit.id == "long").expect("hit");
        assert!(hit.snippet.starts_with('…'));
        assert!(hit.snippet.ends_with('…'));
        assert_eq!(matched_text(hit), "报价");
    }

    #[test]
    fn search_folds_ascii_case_but_keeps_original_text() {
        let connection = search_db();
        seed_bubble(&connection, "case", "chat", "companion", 1, 100, 1, "ABC Quote def");
        let page = search_bubbles(&connection, "chat", "quote", None, None, 50, 0).expect("search");
        assert_eq!(page.total, 1);
        assert_eq!(matched_text(&page.results[0]), "Quote");
    }

    #[test]
    fn search_treats_like_metacharacters_as_literals() {
        let connection = search_db();
        seed_bubble(&connection, "b1", "chat", "companion", 1, 100, 1, "折扣 50% 活动");
        seed_bubble(&connection, "b2", "chat", "companion", 2, 200, 2, "折扣 50X 活动");
        seed_bubble(&connection, "b3", "chat", "companion", 3, 300, 3, "字段 a_b 的值");
        seed_bubble(&connection, "b4", "chat", "companion", 4, 400, 4, "字段 aXb 的值");
        let percent = search_bubbles(&connection, "chat", "50%", None, None, 50, 0).expect("search");
        assert_eq!(percent.results.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(), vec!["b1"]);
        let underscore = search_bubbles(&connection, "chat", "a_b", None, None, 50, 0).expect("search");
        assert_eq!(underscore.results.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(), vec!["b3"]);
        let backslash = search_bubbles(&connection, "chat", "\\", None, None, 50, 0).expect("search");
        assert_eq!(backslash.total, 0);
    }

    #[test]
    fn search_stays_inside_one_private_chat() {
        let connection = search_db();
        connection.execute_batch(
            "INSERT INTO conversations VALUES('other', 'cmp_other');
             INSERT INTO conversations VALUES('normal', NULL);",
        )
        .expect("chats");
        seed_bubble(&connection, "mine", "chat", "companion", 1, 100, 1, "报价单");
        seed_bubble(&connection, "theirs", "other", "companion", 1, 100, 1, "报价单");
        seed_bubble(&connection, "normal-bubble", "normal", "companion", 1, 100, 1, "报价单");
        for (chat, expected) in [("chat", "mine"), ("other", "theirs")] {
            let page = search_bubbles(&connection, chat, "报价", None, None, 50, 0).expect("search");
            assert_eq!(page.results.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(), vec![expected]);
        }
        assert_eq!(search_bubbles(&connection, "normal", "报价", None, None, 50, 0).expect("search").total, 0);
        assert_eq!(search_bubbles(&connection, "missing", "报价", None, None, 50, 0).expect("search").total, 0);
        assert_eq!(search_bubbles(&connection, "chat", "  ", None, None, 50, 0).expect("search").total, 0);
    }

    #[test]
    fn search_filters_dates_and_pages_newest_first() {
        let connection = search_db();
        seed_bubble(&connection, "b1", "chat", "companion", 1, 100, 1, "报价一");
        seed_bubble(&connection, "b2", "chat", "companion", 2, 200, 2, "报价二");
        seed_bubble(&connection, "b3", "chat", "companion", 3, 300, 3, "报价三");
        let all = search_bubbles(&connection, "chat", "报价", None, None, 50, 0).expect("search");
        assert_eq!(all.total, 3);
        assert_eq!(all.results.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(), vec!["b3", "b2", "b1"]);
        let ranged = search_bubbles(&connection, "chat", "报价", Some(150), Some(250), 50, 0).expect("search");
        assert_eq!(ranged.total, 1);
        assert_eq!(ranged.results[0].id, "b2");
        let paged = search_bubbles(&connection, "chat", "报价", None, None, 1, 1).expect("search");
        assert_eq!(paged.total, 3);
        assert_eq!(paged.results[0].id, "b2");
    }

    #[test]
    fn search_hit_locates_a_reply_whose_user_row_is_outside_the_window() {
        let connection = search_db();
        seed_bubble(&connection, "user-1", "chat", "user", 1000, 100, 1, "上次那台投影仪的价格还有效吗？");
        seed_bubble(&connection, "run:response:3", "chat", "companion", 2000, 200, 2, "上次的投影仪报价我存在工作目录里了。");
        let page = search_bubbles(&connection, "chat", "投影仪报价", None, None, 50, 0).expect("search");
        assert_eq!(page.total, 1);
        let hit = &page.results[0];
        assert_eq!(hit.id, "run:response:3");
        assert_eq!(hit.source_ordinal, 2);
        assert_eq!(hit.message_id, "msg-run:response:3");
        // A history window opened at the reply's ordinal cuts the user row but
        // still carries the bubble, so locating never depends on the user row.
        let window = query_window(&connection, "chat", hit.source_ordinal, None).expect("window");
        assert!(window.iter().any(|bubble| bubble.id == hit.id));
        assert!(window.iter().all(|bubble| bubble.author != "user"));
    }
}
