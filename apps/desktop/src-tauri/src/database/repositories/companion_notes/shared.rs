use super::*;
use crate::companion_notes_files::tokens;
use std::collections::BTreeSet;

fn spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn punctuation(c: char) -> bool {
    "，。；！？,.;!?\n".contains(c)
}

fn user_evidence(
    tx: &Connection,
    conversation: &str,
    run: &str,
    source: Option<&str>,
    excerpt: &str,
) -> DbResult<(String, String)> {
    if excerpt.chars().count() < 4 {
        return Err(denied("memory.evidence_required"));
    }
    let (condition, id) = match source {
        Some(id) => ("id", id),
        None => ("run_id", run),
    };
    let mut q=tx.prepare(&format!("SELECT id,content FROM messages WHERE conversation_id=?1 AND role='user' AND {condition}=?2 ORDER BY ordinal DESC"))?;
    for row in q.query_map(params![conversation, id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (mid, body) = row?;
        if spaces(&body).contains(excerpt) {
            return Ok((mid, body));
        }
    }
    Err(denied("memory.evidence_mismatch 请逐字摘录用户原话。当前消息无需传 sourceMessageId，历史消息必须使用实际 ID。"))
}

pub(super) fn direct_statement(body: &str, excerpt: &str, content: &str) -> bool {
    if [
        "这次",
        "这回",
        "本次",
        "今天",
        "这周",
        "这份",
        "暂时",
        "临时",
        "这一次",
    ]
    .iter()
    .any(|w| body.contains(w))
    {
        return false;
    }
    let normalized = spaces(body);
    if let Some((prefix, _)) = normalized.split_once(excerpt) {
        if [
            "客户", "老板", "同事", "他", "她", "对方", "转述", "示例", "举例", "引用",
        ]
        .iter()
        .any(|word| prefix.contains(word))
        {
            return false;
        }
    }
    let phrase = excerpt.trim_end_matches(punctuation);
    if phrase.is_empty() {
        return false;
    }
    let Some(clause) = body
        .split(punctuation)
        .map(spaces)
        .find(|part| part.contains(phrase))
    else {
        return false;
    };
    let clause = clause.trim_start_matches(|c: char| c.is_whitespace());
    let clause = ["不过", "其实", "但是", "但"]
        .iter()
        .find_map(|p| clause.strip_prefix(*p))
        .unwrap_or(clause)
        .trim();
    clause.starts_with("我")
        && ![
            "不", "没", "别", "讨厌", "客户", "老板", "同事", "他", "她", "对方", "说", "引用",
        ]
        .iter()
        .any(|w| clause.contains(w))
        && [
            "喜欢", "习惯", "希望", "偏好", "倾向", "通常", "总是", "叫", "是",
        ]
        .iter()
        .any(|w| clause.contains(w))
        && spaces(content).trim_matches(punctuation) == clause
}

fn subject_words(content: &str) -> BTreeSet<String> {
    let mut text = content.to_owned();
    for word in ["我", "喜欢", "习惯", "希望", "偏好"] {
        text = text.replace(word, " ");
    }
    tokens(&text)
}

pub(super) fn write(
    tx: &Transaction<'_>,
    owner: &str,
    conversation: &str,
    run: &str,
    input: &WriteInput,
) -> DbResult<Value> {
    if input.action != "add" || input.basis.as_deref() != Some("user_stated") {
        return Err(denied("memory.shared_user_statement_required"));
    }
    let kind = input.kind.as_deref().unwrap_or("preference");
    let statement = input.statement.as_deref().unwrap_or("");
    if !matches!(kind, "identity" | "preference" | "fact" | "other")
        || !matches!(
            statement,
            "self_stable" | "self_this_time" | "reported" | "unsure"
        )
    {
        return Err(denied("memory.invalid_shared_input"));
    }
    let text = normalized(input.content.as_deref().unwrap_or(""));
    let key = input.canonical_key.as_deref().unwrap_or("").trim();
    let excerpt = spaces(input.evidence_excerpt.as_deref().unwrap_or(""));
    if key.is_empty() {
        return Err(denied("memory.canonical_key_required 请提供表示偏好主题的 canonicalKey。"));
    }
    if text.is_empty() || text.chars().count() > 1000 {
        return Err(denied("memory.invalid_shared_content"));
    }
    let (mid, message) = user_evidence(
        tx,
        conversation,
        run,
        input.source_message_id.as_deref(),
        &excerpt,
    )?;
    let item = NewMemory {
        owner,
        shared: true,
        kind,
        key,
        content: &text,
        actor: "agent",
        conversation: Some(conversation),
        run: Some(run),
        source_message: Some(&mid),
        evidence: &excerpt,
        basis: Some("user_stated"),
    };
    if let Some(item) = prior(tx, &item, None)? {
        return Ok(item);
    }
    if let Some(id)=tx.query_row("SELECT sr.id FROM companion_shared_requests sr JOIN memory_entities m ON m.id=sr.candidate_memory_id
        WHERE sr.companion_id=?1 AND m.canonical_key=?2 COLLATE NOCASE AND m.content=?3 AND sr.status='open'",params![owner,key,text],|r|r.get::<_,String>(0)).optional()?{
        return Ok(json!({"status":"pending","requestId":id}));
    }
    let count:i64=tx.query_row("SELECT count(*) FROM memory_entities WHERE source_run_id=? AND scope='global' AND source_companion_id IS NOT NULL",[run],|r|r.get(0))?;
    if count >= 3 {
        return Ok(json!({"status":"refused","code":"memory.run_limit"}));
    }
    let words = subject_words(&text);
    let mut stmt=tx.prepare("SELECT id,canonical_key,content,version FROM memory_entities WHERE scope='global' AND kind=? AND state='confirmed' AND enabled=1 AND deleted_at IS NULL")?;
    let mut matches = Vec::new();
    for old in stmt.query_map([kind], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })? {
        let (id, old_key, old_text, version) = old?;
        if old_key.eq_ignore_ascii_case(key) || !words.is_disjoint(&subject_words(&old_text)) {
            matches.push((id, version));
        }
    }
    let confirm = statement == "self_stable"
        && matches.is_empty()
        && !input.changes_existing
        && direct_statement(&message, &excerpt, &text);
    let id = insert(tx, &item, confirm)?;
    if confirm {
        return Ok(json!({"status":"created","memoryId":id}));
    }
    let request = Uuid::new_v4().to_string();
    tx.execute("INSERT INTO companion_shared_requests(id,companion_id,candidate_memory_id,candidate_version,created_at) VALUES (?,?,?,1,?)",
        params![request,owner,id,now_ms()])?;
    for (old, version) in matches {
        tx.execute("INSERT INTO companion_shared_request_matches(request_id,memory_id,expected_version) VALUES (?,?,?)",params![request,old,version])?;
    }
    Ok(json!({"status":"pending","requestId":request,"memoryId":id}))
}

pub(super) fn list(tx: &Connection, owner: &str) -> DbResult<Vec<Value>> {
    json_rows(tx,"SELECT json_object('id',s.id,'candidateMemoryId',m.id,'content',m.content,'evidenceExcerpt',m.evidence_excerpt,
        'matches',json((SELECT json_group_array(json_object('id',x.memory_id,'content',old.content,'expectedVersion',x.expected_version))
        FROM companion_shared_request_matches x LEFT JOIN memory_entities old ON old.id=x.memory_id WHERE x.request_id=s.id)))
        FROM companion_shared_requests s JOIN memory_entities m ON m.id=s.candidate_memory_id
        WHERE s.companion_id=? AND s.status='open' ORDER BY s.created_at",[owner])
}

impl Database {
    pub(crate) fn companion_shared_request_resolve(
        &self,
        owner: &str,
        id: &str,
        decision: &str,
        replace: Option<&str>,
    ) -> Result<Value, String> {
        if !matches!(decision, "add" | "replace" | "reject")
            || (decision == "replace") != replace.is_some()
        {
            return Err("memory.invalid_shared_decision".into());
        }
        self.note_tx(|tx|{
            require_profile(tx,owner)?;
            let(mid,expected):(String,i64)=tx.query_row("SELECT candidate_memory_id,candidate_version FROM companion_shared_requests WHERE id=? AND companion_id=? AND status='open'",
                params![id,owner],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let candidate=load_memory(tx,&mid)?;
            let mut stale=candidate.version!=expected || candidate.deleted_at.is_some() || candidate.state!="candidate";
            let mut replacing=None;
            if let Some(old)=replace {
                let version:i64=tx.query_row("SELECT expected_version FROM companion_shared_request_matches WHERE request_id=? AND memory_id=?",params![id,old],|r|r.get(0))?;
                let row=tx.query_row(&format!("{MEMORY_SELECT} WHERE m.id=? AND m.scope='global' AND m.enabled=1 AND m.state='confirmed' AND m.deleted_at IS NULL"),[old],memory_from_row).optional()?;
                stale|=row.as_ref().map(|m|m.version)!=Some(version);replacing=row;
            }
            let now=now_ms();
            if stale {
                tx.execute("UPDATE companion_shared_requests SET status='stale',resolved_at=? WHERE id=?",params![now,id])?;
                return Ok(json!({"status":"stale","requestId":id}));
            }
            if decision=="reject"{
                remove(tx,&candidate,"user","rejected")?;
                tx.execute("UPDATE companion_shared_requests SET status='rejected',resolved_at=? WHERE id=?",params![now,id])?;
                return Ok(json!({"status":"rejected","requestId":id}));
            }
            if let Some(old)=replacing{
                tx.execute("UPDATE memory_entities SET enabled=0,disabled_at=?1,updated_at=?1,version=version+1 WHERE id=?2",params![now,old.id])?;
                changed(tx,&old,"user","disabled")?;
            }
            tx.execute("UPDATE memory_entities SET state='confirmed',enabled=1,confirmed_at=?1,updated_at=?1,version=version+1 WHERE id=?2",params![now,mid])?;
            changed(tx,&candidate,"user","confirmed")?;
            tx.execute("UPDATE companion_shared_requests SET status=?1,replaced_memory_id=?2,resolved_at=?3 WHERE id=?4",
                params![if replace.is_some(){"replaced"}else{"added"},replace,now,id])?;
            Ok(json!({"status":"updated","memoryId":mid}))
        })
    }
}
