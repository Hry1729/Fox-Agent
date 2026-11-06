use super::{
    memory::{insert_revision, load_memory, memory_from_row, MEMORY_SELECT},
    now_ms, Database,
};
use crate::database::MemoryEntityRecord;
use rusqlite::{params, Connection, OptionalExtension, Params, Transaction};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

mod shared;
mod topics;

type DbResult<T> = rusqlite::Result<T>;

fn denied(code: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(code.into())
}

fn json_rows(db: &Connection, sql: &str, args: impl Params) -> DbResult<Vec<Value>> {
    let mut statement = db.prepare(sql)?;
    let texts = statement.query_map(args, |row| row.get::<_, String>(0))?;
    texts
        .map(|text| serde_json::from_str(&text?).map_err(|e| denied(&e.to_string())))
        .collect()
}

fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_owned()
}

fn owner_notes(db: &Connection, owner: &str) -> DbResult<Vec<MemoryEntityRecord>> {
    let mut stmt = db.prepare(&format!("{MEMORY_SELECT} WHERE scope='agent' AND scope_key=? AND deleted_at IS NULL AND moved_at IS NULL ORDER BY created_at, id"))?;
    let rows = stmt.query_map([owner], memory_from_row)?;
    rows.collect()
}

fn used_chars(db: &Connection, owner: &str) -> DbResult<i64> {
    db.query_row("SELECT coalesce(sum(length(content)),0) FROM memory_entities
        WHERE scope='agent' AND scope_key=? AND enabled=1 AND state='confirmed' AND deleted_at IS NULL AND moved_at IS NULL", [owner], |r| r.get(0))
}

fn over_budget(db: &Connection, owner: &str, delta: i64) -> DbResult<Option<Value>> {
    let over = used_chars(db, owner)? + delta - 4000;
    Ok(if over > 0 {
        Some(
            json!({"status":"refused","code":"companion.note_budget_exceeded","overBy":over,"notes":owner_notes(db,owner)?}),
        )
    } else {
        None
    })
}

fn access(db: &Connection, conversation: &str) -> DbResult<Option<(String, bool)>> {
    db.query_row("SELECT p.id,c.companion_id IS NOT NULL FROM conversations c
        JOIN companion_profiles p ON p.archived_at IS NULL AND p.deleting_at IS NULL AND (
            p.id=c.companion_id OR (c.companion_id IS NULL
              AND EXISTS(SELECT 1 FROM projects pr WHERE pr.id=c.project_id AND pr.status='active')
              AND EXISTS(SELECT 1 FROM companion_project_grants g WHERE g.project_id=c.project_id AND g.companion_id=p.id AND g.revoked_at IS NULL)
              AND EXISTS(SELECT 1 FROM conversation_expert_bindings b WHERE b.conversation_id=c.id AND b.expert_id=p.template_agent_id AND b.state='active')))
        WHERE c.id=? AND c.trashed_at IS NULL", [conversation], |r| Ok((r.get(0)?,r.get(1)?))).optional()
}

fn require_profile(db: &Connection, id: &str) -> DbResult<()> {
    db.query_row(
        "SELECT 1 FROM companion_profiles WHERE id=? AND deleting_at IS NULL",
        [id],
        |_| Ok(()),
    )
}

fn owned(tx: &Transaction<'_>, owner: &str, id: &str) -> DbResult<MemoryEntityRecord> {
    let row = tx.query_row(&format!("{MEMORY_SELECT} WHERE m.id=?1 AND scope='agent' AND scope_key=?2 AND deleted_at IS NULL AND moved_at IS NULL"),
        params![id,owner], memory_from_row).optional()?;
    row.ok_or_else(|| denied("memory.not_found"))
}

fn draft(tx: &Connection, owner: &str, content: &str) -> DbResult<()> {
    tx.execute("INSERT INTO companion_note_drafts(companion_id,content,char_count,updated_at) VALUES (?1,?2,?3,?4)
        ON CONFLICT(companion_id) DO UPDATE SET content=excluded.content,char_count=excluded.char_count,updated_at=excluded.updated_at",
        params![owner,content,normalized(content).chars().count() as i64,now_ms()])?;
    Ok(())
}

fn changed(
    tx: &Transaction<'_>,
    before: &MemoryEntityRecord,
    actor: &str,
    action: &str,
) -> DbResult<MemoryEntityRecord> {
    let after = load_memory(tx, &before.id)?;
    insert_revision(tx, &after, actor, action, Some(before))?;
    Ok(after)
}

fn remove(
    tx: &Transaction<'_>,
    memory: &MemoryEntityRecord,
    actor: &str,
    action: &str,
) -> DbResult<()> {
    tx.execute("UPDATE memory_entities SET deleted_at=?1,disabled_at=?1,enabled=0,updated_at=?1,version=version+1 WHERE id=?2 AND scope=?3 AND scope_key=?4",
        params![now_ms(),memory.id,memory.scope,memory.scope_key])?;
    changed(tx, memory, actor, action)?;
    Ok(())
}

fn was_user_written(db: &Connection, memory: &MemoryEntityRecord) -> DbResult<bool> {
    if memory.created_by == "user" {
        return Ok(true);
    }
    db.query_row("SELECT EXISTS(SELECT 1 FROM memory_revisions WHERE memory_id=? AND actor='user' AND action IN ('created','updated'))",[&memory.id],|r|r.get(0))
}

fn request(
    tx: &Connection,
    owner: &str,
    memory: &MemoryEntityRecord,
    action: &str,
    payload: Value,
    run: &str,
) -> DbResult<Value> {
    let id = Uuid::new_v4().to_string();
    tx.execute("INSERT INTO companion_note_requests(id,companion_id,memory_id,expected_version,action,payload_json,run_id,status,created_at)
        VALUES (?1,?2,?3,?4,?5,?6,?7,'open',?8)",params![id,owner,memory.id,memory.version,action,payload.to_string(),run,now_ms()])?;
    Ok(json!({"status":"pending","requestId":id,"memoryId":memory.id}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WriteInput {
    target: String,
    action: String,
    memory_id: Option<String>,
    kind: Option<String>,
    canonical_key: Option<String>,
    content: Option<String>,
    topic: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    basis: Option<String>,
    statement: Option<String>,
    evidence_excerpt: Option<String>,
    source_message_id: Option<String>,
    #[serde(default)]
    changes_existing: bool,
}

struct NewMemory<'a> {
    owner: &'a str,
    shared: bool,
    kind: &'a str,
    key: &'a str,
    content: &'a str,
    actor: &'a str,
    conversation: Option<&'a str>,
    run: Option<&'a str>,
    source_message: Option<&'a str>,
    evidence: &'a str,
    basis: Option<&'a str>,
}

fn prior(tx: &Connection, item: &NewMemory<'_>, editing: Option<&str>) -> DbResult<Option<Value>> {
    let (scope, scope_key) = if item.shared {
        ("global", "")
    } else {
        ("agent", item.owner)
    };
    if let Some(id)=tx.query_row("SELECT id FROM memory_entities WHERE scope=? AND scope_key=? AND canonical_key=?3 COLLATE NOCASE AND replace(content,char(13)||char(10),char(10))=?4 AND deleted_at IS NULL AND state='confirmed' AND id IS NOT ?5 ORDER BY enabled DESC LIMIT 1",
        params![scope,scope_key,item.key.trim(),item.content,editing],|r|r.get::<_,String>(0)).optional()? {
        return Ok(Some(json!({"status":"existing","memoryId":id})));
    }
    if item.actor=="agent" && tx.query_row("SELECT EXISTS(SELECT 1 FROM memory_entities m JOIN memory_revisions r ON r.memory_id=m.id
      WHERE m.scope=? AND m.scope_key=? AND m.canonical_key=? COLLATE NOCASE AND replace(m.content,char(13)||char(10),char(10))=? AND r.actor='user' AND r.action IN ('deleted','undone'))",
      params![scope,scope_key,item.key.trim(),item.content],|r|r.get::<_,bool>(0))? {
        return Ok(Some(json!({"status":"refused","code":"memory.user_deleted"})));
    }
    Ok(None)
}

fn insert(tx: &Transaction<'_>, item: &NewMemory<'_>, confirmed: bool) -> DbResult<String> {
    let id = Uuid::new_v4().to_string();
    let (scope, key) = if item.shared {
        ("global", "")
    } else {
        ("agent", item.owner)
    };
    let now = now_ms();
    tx.execute("INSERT INTO memory_entities(id,scope,scope_key,kind,canonical_key,content,state,enabled,source_conversation_id,source_run_id,source_message_id,evidence_excerpt,created_by,created_at,updated_at,confirmed_at,source_companion_id,evidence_basis)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14,?15,?16,?17)",
        params![id,scope,key,item.kind,item.key.trim(),item.content,if confirmed{"confirmed"}else{"candidate"},confirmed,item.conversation,item.run,item.source_message,item.evidence,item.actor,now,confirmed.then_some(now),item.owner,item.basis])?;
    let memory = load_memory(tx, &id)?;
    insert_revision(
        tx,
        &memory,
        item.actor,
        if !confirmed {
            "proposed"
        } else if item.actor == "agent" {
            "auto_confirmed"
        } else {
            "created"
        },
        None,
    )?;
    Ok(id)
}

fn save_note(
    tx: &Transaction<'_>,
    item: &NewMemory<'_>,
    before: Option<&MemoryEntityRecord>,
) -> DbResult<Value> {
    if item.content.is_empty() {
        return Err(denied("memory.empty_content"));
    }
    if let Some(existing) = prior(tx, item, before.map(|m| m.id.as_str()))? {
        return Ok(existing);
    }
    let delta = match before {
        Some(m) if m.enabled && m.state == "confirmed" => {
            item.content.chars().count() as i64 - m.content.chars().count() as i64
        }
        Some(_) => 0,
        None => item.content.chars().count() as i64,
    };
    if let Some(refusal) = over_budget(tx, item.owner, delta)? {
        return Ok(refusal);
    }
    let result = if let Some(previous) = before {
        tx.execute("UPDATE memory_entities SET content=?1,canonical_key=?2,kind=?3,updated_at=?4,version=version+1 WHERE id=?5 AND scope='agent' AND scope_key=?6",
            params![item.content,item.key.trim(),item.kind,now_ms(),previous.id,item.owner])?;
        changed(tx, previous, item.actor, "updated")?;
        json!({"status":"updated","memoryId":previous.id})
    } else {
        json!({"status":"created","memoryId":insert(tx,item,true)?})
    };
    Ok(result)
}

impl Database {
    fn note_tx<T>(&self, f: impl FnOnce(&Transaction<'_>) -> DbResult<T>) -> Result<T, String> {
        self.with_connection(|db| {
            let tx = db.transaction()?;
            let result = f(&tx)?;
            tx.commit()?;
            Ok(result)
        })
    }

    pub(crate) fn companion_note_access(
        &self,
        conversation_id: &str,
    ) -> Result<Option<(String, bool)>, String> {
        self.with_connection(|db| access(db, conversation_id))
    }

    pub(crate) fn companion_notes_get(&self, id: &str) -> Result<Value, String> {
        self.note_tx(|tx|{
            require_profile(tx,id)?;
            let requests=json_rows(tx,"SELECT json_object('id',q.id,'memoryId',q.memory_id,'expectedVersion',q.expected_version,'action',q.action,'payload',json(q.payload_json),'content',m.content)
                FROM companion_note_requests q JOIN memory_entities m ON m.id=q.memory_id WHERE q.companion_id=? AND q.status='open' ORDER BY q.created_at",[id])?;
            let moves=json_rows(tx,"SELECT json_object('id',id,'topicId',topic_id,'memoryIds',json(memory_ids_json),'state',state,'createdAt',created_at)
                FROM companion_note_moves WHERE companion_id=? ORDER BY created_at DESC",[id])?;
            let draft:Option<String>=tx.query_row("SELECT content FROM companion_note_drafts WHERE companion_id=?",[id],|r|r.get(0)).optional()?;
            Ok(json!({"notes":owner_notes(tx,id)?,"usedChars":used_chars(tx,id)?,"limit":4000,"draft":draft,"topics":topics::list(tx,id)?,
                "moves":moves,"requests":requests,"sharedRequests":shared::list(tx,id)?}))
        })
    }

    pub(crate) fn companion_note_save_user(
        &self,
        id: &str,
        memory_id: Option<&str>,
        version: Option<i64>,
        content: &str,
        key: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Value, String> {
        self.note_tx(|tx| {
            require_profile(tx, id)?;
            let before = memory_id.map(|mid| owned(tx, id, mid)).transpose()?;
            if before.as_ref().is_some_and(|m| Some(m.version) != version) {
                return Err(denied("memory.stale_or_deleted"));
            }
            let text = normalized(content);
            let generated = Uuid::new_v4().to_string();
            let item = NewMemory {
                owner: id,
                shared: false,
                actor: "user",
                kind: kind
                    .or_else(|| before.as_ref().map(|m| m.kind.as_str()))
                    .unwrap_or("fact"),
                key: key
                    .or_else(|| before.as_ref().map(|m| m.canonical_key.as_str()))
                    .unwrap_or(&generated),
                content: &text,
                conversation: None,
                run: None,
                source_message: None,
                evidence: "",
                basis: None,
            };
            let result = save_note(tx, &item, before.as_ref())?;
            if result["overBy"].is_number() {
                draft(tx, id, content)?;
            } else {
                tx.execute(
                    "DELETE FROM companion_note_drafts WHERE companion_id=? AND content=?",
                    params![id, content],
                )?;
            }
            Ok(result)
        })
    }

    pub(crate) fn companion_note_draft_save(&self, id: &str, text: &str) -> Result<(), String> {
        self.note_tx(|tx| {
            require_profile(tx, id)?;
            draft(tx, id, text)
        })
    }

    pub(crate) fn companion_note_set_enabled(
        &self,
        id: &str,
        mid: &str,
        version: i64,
        enabled: bool,
    ) -> Result<Value, String> {
        self.note_tx(|tx|{
            let m=owned(tx,id,mid)?;
            if m.version!=version || m.state!="confirmed" {return Err(denied("memory.stale_or_not_confirmed"))}
            if enabled && !m.enabled {if let Some(v)=over_budget(tx,id,m.content.chars().count() as i64)? {return Ok(v)}}
            tx.execute("UPDATE memory_entities SET enabled=?1,disabled_at=CASE WHEN ?1 THEN NULL ELSE ?2 END,updated_at=?2,version=version+1 WHERE id=?3 AND scope_key=?4",
                        params![enabled,now_ms(),mid,id])?;
            changed(tx,&m,"user",if enabled{"enabled"}else{"disabled"})?;
            Ok(json!({"status":"updated","memoryId":mid}))
        })
    }

    pub(crate) fn companion_note_delete(
        &self,
        id: &str,
        mid: Option<&str>,
        version: Option<i64>,
    ) -> Result<Value, String> {
        self.note_tx(|tx| {
            require_profile(tx, id)?;
            let notes = if let Some(mid) = mid {
                let m = owned(tx, id, mid)?;
                if Some(m.version) != version {
                    return Err(denied("memory.stale_or_deleted"));
                }
                vec![m]
            } else {
                owner_notes(tx, id)?
            };
            for m in &notes {
                remove(tx, m, "user", "deleted")?;
            }
            Ok(json!({"status":"removed","count":notes.len()}))
        })
    }

    pub(crate) fn companion_note_confirm(
        &self,
        id: &str,
        mid: &str,
        version: i64,
    ) -> Result<Value, String> {
        self.note_tx(|tx|{
            let m=owned(tx,id,mid)?;
            if m.version!=version || m.state!="candidate" || m.open_conflict_id.is_some() {return Err(denied("memory.stale_or_invalid_transition"))}
            if let Some(v)=over_budget(tx,id,m.content.chars().count() as i64)? {return Ok(v)}
            tx.execute("UPDATE memory_entities SET state='confirmed',enabled=1,confirmed_at=?1,updated_at=?1,version=version+1 WHERE id=?2 AND scope_key=?3",
                params![now_ms(),mid,id])?;
            changed(tx,&m,"user","confirmed")?;
            Ok(json!({"status":"updated","memoryId":mid}))
        })
    }

    pub(crate) fn companion_memory_undo(
        &self,
        id: &str,
        mid: &str,
        run: &str,
    ) -> Result<Value, String> {
        self.note_tx(|tx| {
            let memory=tx.query_row(&format!("{MEMORY_SELECT} WHERE m.id=?1 AND source_companion_id=?2
                AND ((scope='agent' AND scope_key=?2) OR scope='global')"),
                params![mid,id], memory_from_row).optional()?.ok_or_else(||denied("memory.not_found"))?;
            let single:bool=tx.query_row("SELECT count(*)=1 AND max(action)='auto_confirmed' FROM memory_revisions WHERE memory_id=?",
                [mid], |r|r.get(0))?;
            if !single || memory.created_by!="agent" || memory.version!=1 || memory.source_run_id.as_deref()!=Some(run) || memory.deleted_at.is_some() {
                return Err(denied("memory.undo_not_available"));
            }
            remove(tx,&memory,"user","undone")?;
            Ok(json!({"status":"removed","memoryId":mid}))
        })
    }

    pub(crate) fn companion_grants_list(&self, id: &str) -> Result<Value, String> {
        self.with_connection(|db|Ok(json!(json_rows(db,"SELECT json_object('id',g.id,'projectId',p.id,'projectName',p.name,'grantedAt',g.granted_at)
            FROM companion_project_grants g JOIN projects p ON p.id=g.project_id WHERE g.companion_id=? AND g.revoked_at IS NULL ORDER BY p.name",[id])?)))
    }

    pub(crate) fn companion_grant_project(
        &self,
        id: &str,
        project: &str,
        grant: bool,
    ) -> Result<(), String> {
        self.note_tx(|tx|{
            if grant {tx.execute("INSERT INTO companion_project_grants(id,project_id,companion_id,granted_at)
                VALUES (?,?,?,?) ON CONFLICT(project_id,companion_id) WHERE revoked_at IS NULL DO NOTHING",params![Uuid::new_v4().to_string(),project,id,now_ms()])?;}
            else {tx.execute("UPDATE companion_project_grants SET revoked_at=? WHERE project_id=? AND companion_id=? AND revoked_at IS NULL",params![now_ms(),project,id])?;}
            Ok(())
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Organize {
    memory_ids: Vec<String>,
    topic: String,
    #[serde(default)]
    aliases: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    query: String,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    topic: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

fn input<T: serde::de::DeserializeOwned>(value: &Value) -> DbResult<T> {
    serde_json::from_value(value.clone())
        .map_err(|e| denied(&format!("companion.invalid_tool_input {e}")))
}

impl Database {
    pub(crate) fn companion_runtime_notes(
        &self,
        conversation_id: &str,
        query: &str,
    ) -> Result<Option<Value>, String> {
        self.note_tx(|tx|{
            let Some((owner,writable))=access(tx,conversation_id)? else{return Ok(None)};
            let(name,duty,persona):(String,String,Option<String>)=tx.query_row("SELECT display_name,duty,persona_override FROM companion_profiles WHERE id=?",
                [&owner],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let notes:Vec<_>=owner_notes(tx,&owner)?.into_iter().filter(|m|m.state=="confirmed"&&m.enabled)
                .map(|m|json!({"id":m.id,"content":m.content})).collect();
            let hits=topics::search(tx,&owner,query,3)?;
            let mut remaining=1500usize;let mut snippets=Vec::new();
            for mut hit in hits {
                if remaining==0{break}
                let text:String=hit["text"].as_str().unwrap_or_default().chars().take(remaining).collect();
                remaining-=text.chars().count();hit["text"]=Value::String(text);snippets.push(hit);
            }
            let mut available=2000;let mut topics=Vec::new();
            for row in topics::list(tx,&owner)?.into_iter().take(40){
                let entry=json!({"id":row["id"],"title":row["title"],"aliases":row["aliases"],"fileName":row["fileName"]});
                let size=entry.to_string().chars().count(); if size>available{break} available-=size;topics.push(entry);
            }
            Ok(Some(json!({"role":if writable{"owner"}else{"reader"},"companionId":owner,"displayName":name,
                "duty":if writable{Some(duty)}else{None},"personaOverride":if writable{persona}else{None},"usedChars":used_chars(tx,&owner)?,
                "notes":notes,"topics":topics,"snippets":snippets})))
        })
    }

    pub(crate) fn companion_notes_tool(
        &self,
        conversation_id: &str,
        run_id: &str,
        tool: &str,
        value: &Value,
    ) -> Result<Value, String> {
        self.note_tx(|tx|{
            let(owner,writable)=access(tx,conversation_id)?.ok_or_else(||denied("companion.tool_not_available"))?;
            let belongs:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE id=? AND conversation_id=?)",params![run_id,conversation_id],|r|r.get(0))?;
            if !belongs{return Err(denied("companion.tool_not_available"))}
            match tool {
                "note_search"=>{let args:Search=input(value)?;Ok(json!({"items":topics::search(tx,&owner,&args.query,args.limit.unwrap_or(8).clamp(1,20))?}))},
                "note_read"=>{let args:Read=input(value)?;topics::read(tx,&owner,&args.topic,args.offset.unwrap_or(0),args.limit.unwrap_or(8000).clamp(1,8000))},
                "note_organize" if writable=>{let args:Organize=input(value)?;topics::organize(tx,&owner,run_id,&args.memory_ids,&args.topic,&args.aliases,false)},
                "memory_write" if writable=>{
                    let args:WriteInput=input(value)?;
                    match args.target.as_str() {
                        "shared"=>shared::write(tx,&owner,conversation_id,run_id,&args),
                        "topic" if args.action=="add"=>{
                            let text=normalized(args.content.as_deref().unwrap_or(""));
                            let topic=args.topic.as_deref().filter(|t|!t.trim().is_empty()).ok_or_else(||denied("companion.topic_required"))?;
                            if text.is_empty(){return Err(denied("memory.empty_content"))}
                            topics::append(tx,&owner,topic,&args.aliases,&text,"agent")
                        },
                        "notes"=>agent_note(tx,&owner,conversation_id,run_id,&args),
                        _=>Err(denied("companion.invalid_tool_input")),
                    }
                }
                _=>Err(denied("companion.tool_not_available")),
            }
        })
    }

    pub(crate) fn companion_note_request_resolve(
        &self,
        owner: &str,
        id: &str,
        approve: bool,
    ) -> Result<Value, String> {
        self.note_tx(|tx|{
            let(mid,version,action,body,run):(String,i64,String,String,String)=tx.query_row("SELECT memory_id,expected_version,action,payload_json,coalesce(run_id,'') FROM companion_note_requests WHERE id=? AND companion_id=? AND status='open'",
                params![id,owner],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            let data:Value=serde_json::from_str(&body).map_err(|e|denied(&e.to_string()))?;
            let mark=|status:&str|tx.execute("UPDATE companion_note_requests SET status=?,resolved_at=? WHERE id=? AND companion_id=?",params![status,now_ms(),id,owner]);
            if !approve {mark("rejected")?;return Ok(json!({"status":"rejected","requestId":id}));}
            let m=load_memory(tx,&mid)?;
            if m.scope!="agent"||m.scope_key!=owner||m.version!=version||m.deleted_at.is_some(){mark("stale")?;return Ok(json!({"status":"stale","requestId":id}));}
            let result=match action.as_str(){
                "remove"=>{remove(tx,&m,"user","deleted")?;json!({"status":"removed","memoryId":mid})},
                "patch"=>{
                    let item=NewMemory{owner,shared:false,actor:"agent",content:data["content"].as_str().ok_or_else(||denied("memory.empty_content"))?,
                        key:data["canonicalKey"].as_str().unwrap_or(&m.canonical_key),kind:data["kind"].as_str().unwrap_or(&m.kind),
                        conversation:None,run:Some(&run),source_message:None,evidence:"",basis:None};
                    save_note(tx,&item,Some(&m))?
                },
                "move"=>{let aliases:Vec<String>=input(&data["aliases"])?;
                    topics::organize(tx,owner,&run,&[mid],data["topic"].as_str().ok_or_else(||denied("companion.topic_required"))?,&aliases,true)?},
                _=>return Err(denied("companion.invalid_request")),
            };
            if result["status"]!="refused" {mark("approved")?;}
            Ok(result)
        })
    }

    pub(crate) fn companion_note_conflict_resolve(
        &self,
        owner: &str,
        id: &str,
        accept: bool,
    ) -> Result<Value, String> {
        self.note_tx(|tx|{
            let(old_id,new_id):(String,String)=tx.query_row("SELECT existing_memory_id,competing_memory_id FROM memory_conflicts WHERE id=? AND status='open'",[id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let old=owned(tx,owner,&old_id)?;let new=owned(tx,owner,&new_id)?;
            let now=now_ms();
            if accept {
                let freed=if old.enabled && old.state=="confirmed"{old.content.chars().count()as i64}else{0};
                if let Some(v)=over_budget(tx,owner,new.content.chars().count()as i64-freed)?{return Ok(v)}
                tx.execute("UPDATE memory_entities SET enabled=0,disabled_at=?1,updated_at=?1,version=version+1 WHERE id=?2 AND scope_key=?3",params![now,old_id,owner])?;
                changed(tx,&old,"user","disabled")?;
                tx.execute("UPDATE memory_entities SET state='confirmed',enabled=1,confirmed_at=?1,updated_at=?1,version=version+1 WHERE id=?2 AND scope_key=?3",params![now,new_id,owner])?;
            } else {
                tx.execute("UPDATE memory_entities SET state='candidate',enabled=0,disabled_at=?1,updated_at=?1,version=version+1 WHERE id=?2 AND scope_key=?3",params![now,new_id,owner])?;
            }
            changed(tx,&new,"user",if accept{"confirmed"}else{"disabled"})?;
            tx.execute("UPDATE memory_conflicts SET status='resolved',resolution=?,resolved_at=? WHERE id=?",params![if accept{"accept_competing"}else{"keep_existing"},now,id])?;
            Ok(json!({"status":"updated","memoryId":if accept{new_id}else{old_id}}))
        })
    }
}

fn agent_note(
    tx: &Transaction<'_>,
    owner: &str,
    conversation: &str,
    run: &str,
    args: &WriteInput,
) -> DbResult<Value> {
    let action = args.action.as_str();
    let before = match action {
        "add" => None,
        "patch" | "remove" => Some(owned(
            tx,
            owner,
            args.memory_id
                .as_deref()
                .ok_or_else(|| denied("memory.id_required"))?,
        )?),
        _ => return Err(denied("companion.invalid_tool_input")),
    };
    let content = normalized(args.content.as_deref().unwrap_or(""));
    if action != "remove" && content.is_empty() {
        return Err(denied("memory.empty_content"));
    }
    let kind = args
        .kind
        .as_deref()
        .or_else(|| before.as_ref().map(|m| m.kind.as_str()))
        .unwrap_or("fact");
    let key = args
        .canonical_key
        .as_deref()
        .or_else(|| before.as_ref().map(|m| m.canonical_key.as_str()))
        .unwrap_or(&content);
    if let Some(m) = &before {
        if was_user_written(tx, m)? {
            return request(
                tx,
                owner,
                m,
                action,
                json!({"content":content,"canonicalKey":key,"kind":kind}),
                run,
            );
        }
        if action == "remove" {
            remove(tx, m, "agent", "deleted")?;
            return Ok(json!({"status":"removed","memoryId":m.id}));
        }
    }
    let item = NewMemory {
        owner,
        shared: false,
        actor: "agent",
        kind,
        key,
        content: &content,
        conversation: Some(conversation),
        run: Some(run),
        source_message: None,
        evidence: args.evidence_excerpt.as_deref().unwrap_or(""),
        basis: args.basis.as_deref(),
    };
    save_note(tx, &item, before.as_ref())
}

#[cfg(test)]
mod tests;
