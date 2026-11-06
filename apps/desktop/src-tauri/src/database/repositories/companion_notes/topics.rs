use super::*;
use crate::companion_notes_files as files;
use std::{
    fs,
    path::{Path, PathBuf},
};

const LIST: &str =
    "SELECT json_object('id',id,'title',title,'aliases',json(aliases_json),'fileName',file_name,
    'revision',revision,'contentSha256',content_sha256,'charCount',char_count)
    FROM companion_note_topics WHERE companion_id=? AND deleted_at IS NULL ORDER BY title,id";

pub(super) fn list(tx: &Transaction<'_>, owner: &str) -> DbResult<Vec<Value>> {
    json_rows(tx, LIST, [owner])
}

struct Topic {
    id: String,
    owner: String,
    title: String,
    aliases: Vec<String>,
    path: PathBuf,
    revision: i64,
    sha: String,
    raw: String,
}

impl Topic {
    fn body(&self) -> String {
        files::parse(&self.raw).1
    }
    fn view(&self, offset: usize, limit: usize) -> Value {
        let body = self.body();
        let content: String = body.chars().skip(offset).take(limit).collect();
        json!({"id":self.id,"title":self.title,"aliases":self.aliases,"revision":self.revision,"contentSha256":self.sha,
            "content":content,"offset":offset,"nextOffset":offset+content.chars().count(),"hasMore":offset+content.chars().count()<body.chars().count()})
    }
}

fn io_error(err: std::io::Error) -> rusqlite::Error {
    denied(&format!("companion.topic_file {err}"))
}

fn path(tx: &Connection, owner: &str, file: &str) -> DbResult<PathBuf> {
    let db = tx
        .path()
        .and_then(|p| Path::new(p).parent())
        .ok_or_else(|| denied("companion.notes_dir_unavailable"))?;
    files::topic_path(db, owner, file).map_err(io_error)
}

fn locate(tx: &Connection, owner: &str, name: &str) -> DbResult<Option<Topic>> {
    let row=tx.query_row("SELECT id,title,aliases_json,file_name,revision,content_sha256 FROM companion_note_topics
        WHERE companion_id=?1 AND deleted_at IS NULL AND (id=?2 OR title=?2 COLLATE NOCASE OR file_name=?2)",
        params![owner,name],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?))).optional()?;
    let Some((id, title, aliases, file, revision, sha)) = row else {
        return Ok(None);
    };
    let path = path(tx, owner, &file)?;
    let raw = fs::read_to_string(&path).map_err(io_error)?;
    let mut t = Topic {
        id,
        owner: owner.into(),
        title,
        aliases: serde_json::from_str(&aliases).map_err(|e| denied(&e.to_string()))?,
        path,
        revision,
        sha,
        raw,
    };
    let actual = files::sha(&t.raw);
    if actual != t.sha {
        if let Some(h) = files::parse(&t.raw).0 {
            t.title = h.title;
            t.aliases = h.aliases;
        }
        t.sha = actual;
        t.revision += 1;
        index(tx, &t)?;
    }
    Ok(Some(t))
}

fn index(tx: &Connection, t: &Topic) -> DbResult<()> {
    let body = t.body();
    tx.execute("UPDATE companion_note_topics SET title=?1,aliases_json=?2,revision=?3,content_sha256=?4,char_count=?5,updated_at=?6 WHERE id=?7 AND companion_id=?8 AND deleted_at IS NULL",
        params![t.title,json!(t.aliases).to_string(),t.revision,t.sha,body.chars().count() as i64,now_ms(),t.id,t.owner])?;
    tx.execute(
        "DELETE FROM companion_note_chunks WHERE topic_id=?",
        [&t.id],
    )?;
    for (i, (heading, text)) in files::chunks(&body).into_iter().enumerate() {
        let tokens = files::tokens(&text)
            .into_iter()
            .collect::<Vec<_>>()
            .join(" ");
        tx.execute("INSERT INTO companion_note_chunks(topic_id,ordinal,heading,text,bigrams,topic_revision) VALUES (?,?,?,?,?,?)",
            params![t.id,i as i64,heading,text,tokens,t.revision])?;
    }
    Ok(())
}

fn create(
    tx: &Connection,
    owner: &str,
    title: &str,
    aliases: &[String],
    actor: &str,
) -> DbResult<Topic> {
    let id = format!("ntp_{}", Uuid::new_v4().simple());
    let file = format!("{id}.md");
    let path = path(tx, owner, &file)?;
    tx.execute("INSERT INTO companion_note_topics(id,companion_id,title,file_name,aliases_json,created_by,revision,content_sha256,created_at,updated_at)
        VALUES (?1,?2,?3,?4,?5,?6,0,'',?7,?7)", params![id,owner,title,file,json!(aliases).to_string(),actor,now_ms()])?;
    Ok(Topic {
        id,
        owner: owner.into(),
        title: title.into(),
        aliases: aliases.to_vec(),
        path,
        revision: 0,
        sha: String::new(),
        raw: String::new(),
    })
}

fn write(tx: &Connection, topic: &mut Topic, raw: String) -> DbResult<()> {
    topic.raw = raw;
    topic.revision += 1;
    topic.sha = files::sha(&topic.raw);
    index(tx, topic)?;
    files::replace(&topic.path, &topic.raw).map_err(io_error)
}

pub(super) fn read(
    tx: &Connection,
    owner: &str,
    name: &str,
    offset: usize,
    limit: usize,
) -> DbResult<Value> {
    let topic = locate(tx, owner, name)?.ok_or_else(|| denied("companion.topic_not_found"))?;
    Ok(topic.view(offset, limit))
}

pub(super) fn search(
    tx: &Transaction<'_>,
    owner: &str,
    query: &str,
    limit: usize,
) -> DbResult<Vec<Value>> {
    let query = files::tokens(query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    for t in list(tx, owner)? {
        locate(tx, owner, t["id"].as_str().unwrap_or_default())?;
    }
    let mut stmt=tx.prepare("SELECT t.id,t.title,t.aliases_json,c.heading,c.text,c.bigrams FROM companion_note_chunks c
        JOIN companion_note_topics t ON t.id=c.topic_id AND t.deleted_at IS NULL WHERE t.companion_id=? ORDER BY t.id,c.ordinal")?;
    let rows = stmt.query_map([owner], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut hits = Vec::new();
    for row in rows {
        let (id, title, aliases, heading, text, stored) = row?;
        let score = stored
            .split_whitespace()
            .filter(|w| query.contains(*w))
            .count()
            + 3 * query.intersection(&files::tokens(&title)).count()
            + 2 * query.intersection(&files::tokens(&aliases)).count()
            + query.intersection(&files::tokens(&heading)).count();
        if score > 0 {
            hits.push((
                score,
                json!({"topicId":id,"title":title,"heading":heading,"text":text,"score":score}),
            ));
        }
    }
    hits.sort_by_key(|p| std::cmp::Reverse(p.0));
    Ok(hits.into_iter().take(limit).map(|(_, v)| v).collect())
}

// A missing title may create a topic. An ID or managed filename can only select an existing one.
fn write_target(tx: &Connection, owner: &str, name: &str) -> DbResult<Option<Topic>> {
    let topic = locate(tx, owner, name)?;
    let identifier = name
        .strip_suffix(".md")
        .unwrap_or(name)
        .strip_prefix("ntp_")
        .and_then(|suffix| Uuid::parse_str(suffix).ok());
    if topic.is_none() && identifier.is_some() {
        return Err(denied("companion.topic_not_found"));
    }
    Ok(topic)
}

pub(super) fn append(
    tx: &Transaction<'_>,
    owner: &str,
    title: &str,
    aliases: &[String],
    content: &str,
    actor: &str,
) -> DbResult<Value> {
    let mut t = match write_target(tx, owner, title)? {
        Some(t) => t,
        None => create(tx, owner, title, aliases, actor)?,
    };
    let body = format!("{}\n\n{}", t.body(), content);
    let raw = files::render(&t.title, &t.aliases, body.trim_start_matches('\n'));
    write(tx, &mut t, raw)?;
    Ok(json!({"status":"updated","topicId":t.id,"contentSha256":t.sha}))
}

pub(super) fn organize(
    tx: &Transaction<'_>,
    owner: &str,
    run: &str,
    ids: &[String],
    topic: &str,
    aliases: &[String],
    approved: bool,
) -> DbResult<Value> {
    if ids.is_empty() || ids.len() > 20 {
        return Err(denied("companion.invalid_move_count"));
    }
    let target = write_target(tx, owner, topic)?;
    let mut movable = Vec::new();
    let mut requests = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(denied("companion.duplicate_move"));
        }
        let m = owned(tx, owner, id)?;
        if !approved && was_user_written(tx, &m)? {
            requests.push(request(
                tx,
                owner,
                &m,
                "move",
                json!({"topic":topic,"aliases":aliases}),
                run,
            )?);
        } else {
            movable.push(m);
        }
    }
    if movable.is_empty() {
        return Ok(json!({"status":"pending","requests":requests}));
    }
    let mut t = match target {
        Some(t) => t,
        None => create(tx, owner, topic, aliases, "agent")?,
    };
    let move_id = Uuid::new_v4().to_string();
    let before = if t.revision > 0 {
        Some(t.sha.clone())
    } else {
        None
    };
    let original = if t.revision > 0 {
        t.raw.clone()
    } else {
        files::render(&t.title, &t.aliases, "")
    };
    let mut raw = format!("{original}\n\n<!-- fox-move {move_id} -->\n");
    let now = now_ms();
    // Keep the failure record even when the file cannot be replaced.
    tx.execute_batch("SAVEPOINT companion_move_changes")?;
    let mut moved = Vec::new();
    for m in &movable {
        raw.push_str(&format!(
            "<!-- fox-note id={} at={} by={} -->\n{}\n\n",
            m.id, now, m.created_by, m.content
        ));
        tx.execute("UPDATE memory_entities SET moved_at=?1,moved_topic_id=?2,version=version+1,updated_at=?1 WHERE id=?3 AND scope='agent' AND scope_key=?4",
            params![now,t.id,m.id,owner])?;
        changed(tx, m, if approved { "user" } else { "agent" }, "moved")?;
        moved.push(&m.id);
    }
    let after = files::sha(&raw);
    tx.execute("INSERT INTO companion_note_moves(id,companion_id,topic_id,run_id,memory_ids_json,topic_sha_before,topic_sha_after,state,created_at)
        VALUES (?,?,?,?,?,?,?,'applied',?)",params![move_id,owner,t.id,run,json!(moved).to_string(),before,after,now])?;
    if let Err(error) = write(tx, &mut t, raw) {
        if !matches!(&error, rusqlite::Error::InvalidParameterName(code) if code.starts_with("companion.topic_file"))
        {
            return Err(error);
        }
        tx.execute_batch("ROLLBACK TO companion_move_changes; RELEASE companion_move_changes")?;
        if before.is_none() {
            tx.execute(
                "UPDATE companion_note_topics SET deleted_at=? WHERE id=?",
                params![now, t.id],
            )?;
        }
        tx.execute("INSERT INTO companion_note_moves(id,companion_id,topic_id,run_id,memory_ids_json,topic_sha_before,state,error,created_at)
            VALUES (?,?,?,?,?,?,'failed',?,?)", params![move_id,owner,t.id,run,json!(moved).to_string(),before,error.to_string(),now])?;
        return Ok(
            json!({"status":"refused","code":"companion.topic_file","error":error.to_string(),"moveId":move_id,"requests":requests}),
        );
    }
    tx.execute_batch("RELEASE companion_move_changes")?;
    Ok(
        json!({"status":"updated","topicId":t.id,"moveId":move_id,"moved":moved,"requests":requests}),
    )
}

impl Database {
    pub(crate) fn cleanup_companion_topic_files(&self, data_dir: &Path) -> Result<(), String> {
        self.with_connection(|db| {
            let mut stmt=db.prepare("SELECT id FROM companion_profiles WHERE deleting_at IS NULL")?;
            let ids=stmt.query_map([],|r|r.get::<_,String>(0))?;
            for owner in ids {
                let owner = owner?;
                let dir=match files::topics_dir(data_dir,&owner){
                    Ok(dir)=>dir,
                    Err(e) if e.kind()==std::io::ErrorKind::NotFound=>continue,
                    Err(e)=>return Err(io_error(e)),
                };
                for entry in fs::read_dir(dir).map_err(io_error)? {
                    let entry=entry.map_err(io_error)?;
                    let name=entry.file_name();
                    let Some(name)=name.to_str() else {continue};
                    let managed=name.strip_prefix("ntp_").and_then(|s|s.strip_suffix(".md")).and_then(|s|Uuid::parse_str(s).ok()).is_some();
                    if !managed {continue;}
                    let live:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM companion_note_topics WHERE companion_id=? AND file_name=? AND deleted_at IS NULL)",
                        params![owner,name],|r|r.get(0))?;
                    if !live {
                        let file=files::topic_path(data_dir,&owner,name).map_err(io_error)?;
                        fs::remove_file(file).map_err(io_error)?;
                    }
                }
            }
            Ok(())
        })
    }

    pub(crate) fn companion_topic_read(&self, owner: &str, name: &str) -> Result<Value, String> {
        self.note_tx(|tx| {
            require_profile(tx, owner)?;
            read(tx, owner, name, 0, usize::MAX)
        })
    }
    pub(crate) fn companion_topic_save(
        &self,
        owner: &str,
        id: Option<&str>,
        title: &str,
        aliases: &[String],
        body: &str,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.note_tx(|tx| {
            require_profile(tx, owner)?;
            let mut t = if let Some(id) = id {
                let t =
                    locate(tx, owner, id)?.ok_or_else(|| denied("companion.topic_not_found"))?;
                if Some(t.sha.as_str()) != expected {
                    return Ok(
                        json!({"status":"conflict","current":t.view(0,usize::MAX),"draft":body}),
                    );
                }
                t
            } else {
                create(tx, owner, title, aliases, "user")?
            };
            t.title = title.trim().into();
            t.aliases = aliases.to_vec();
            write(tx, &mut t, files::render(title, aliases, body))?;
            Ok(json!({"status":"updated","topicId":t.id,"contentSha256":t.sha}))
        })
    }

    pub(crate) fn companion_note_draft_to_topic(
        &self,
        owner: &str,
        title: &str,
        aliases: &[String],
    ) -> Result<Value, String> {
        self.note_tx(|tx| {
            require_profile(tx, owner)?;
            let content: String = tx.query_row(
                "SELECT content FROM companion_note_drafts WHERE companion_id=?",
                [owner],
                |r| r.get(0),
            )?;
            tx.execute(
                "DELETE FROM companion_note_drafts WHERE companion_id=?",
                [owner],
            )?;
            append(tx, owner, title, aliases, &content, "user")
        })
    }

    pub(crate) fn companion_topic_delete(
        &self,
        owner: &str,
        id: &str,
        expected: &str,
    ) -> Result<Value, String> {
        let (view,path)=self.note_tx(|tx|{
            require_profile(tx,owner)?;
            let t=locate(tx,owner,id)?.ok_or_else(||denied("companion.topic_not_found"))?;
            if t.sha!=expected{return Ok((json!({"status":"conflict","current":t.view(0,usize::MAX)}),None))}
            let mut stmt=tx.prepare(&format!("{MEMORY_SELECT} WHERE m.scope='agent' AND m.scope_key=? AND m.moved_topic_id=? AND m.deleted_at IS NULL"))?;
            let rows=stmt.query_map(params![owner,t.id],memory_from_row)?.collect::<DbResult<Vec<_>>>()?;
            tx.execute("UPDATE companion_note_topics SET deleted_at=?1,updated_at=?1 WHERE id=?2 AND companion_id=?3",params![now_ms(),t.id,owner])?;
            for m in rows {
                tx.execute("UPDATE memory_entities SET version=version+1,updated_at=?1 WHERE id=?2 AND scope_key=?3",params![now_ms(),m.id,owner])?;
                changed(tx,&m,"user","deleted")?;
            }
            Ok((json!({"status":"removed","topicId":t.id}),Some(t.path)))
        })?;
        if let Some(p) = path {
            if let Err(e) = fs::remove_file(p) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Ok(json!({"status":"removed","fileCleanupPending":true,"topicId":id}));
                }
            }
        }
        Ok(view)
    }

    pub(crate) fn companion_note_move_undo(&self, owner: &str, id: &str) -> Result<Value, String> {
        self.note_tx(|tx|{
            let(topic,ids,expected):(String,String,String)=tx.query_row("SELECT topic_id,memory_ids_json,topic_sha_after FROM companion_note_moves
                WHERE id=? AND companion_id=? AND state='applied'",params![id,owner],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let mut t=locate(tx,owner,&topic)?.ok_or_else(||denied("companion.topic_not_found"))?;
            if t.sha!=expected{return Err(denied("companion.topic_changed"))}
            let ids:Vec<String>=serde_json::from_str(&ids).map_err(|e|denied(&e.to_string()))?;
            let mut delta=0;
            for mid in &ids{
                delta+=tx.query_row("SELECT CASE WHEN state='confirmed' AND enabled=1 THEN length(content) ELSE 0 END FROM memory_entities
                        WHERE id=?1 AND scope='agent' AND scope_key=?2 AND deleted_at IS NULL AND moved_topic_id=?3 AND moved_at IS NOT NULL",params![mid,owner,topic],|r|r.get::<_,i64>(0))?;
            }
            if let Some(v)=over_budget(tx,owner,delta)?{return Ok(v)}
            let marker=format!("\n\n<!-- fox-move {id} -->\n");
            let raw=t.raw.rsplit_once(&marker).ok_or_else(||denied("companion.topic_changed"))?.0.to_owned();
            for mid in &ids{
                let m=load_memory(tx,mid)?;
                tx.execute("UPDATE memory_entities SET moved_at=NULL,moved_topic_id=NULL,version=version+1,updated_at=?1 WHERE id=?2 AND scope_key=?3",params![now_ms(),mid,owner])?;
                changed(tx,&m,"user","move_undone")?;
            }
            tx.execute("UPDATE companion_note_moves SET state='undone',undone_at=? WHERE id=? AND companion_id=?",params![now_ms(),id,owner])?;
            write(tx,&mut t,raw)?;
            Ok(json!({"status":"updated","moveId":id}))
        })
    }
}
